//! #3312 pins — the argument-position `from` of the callable-class `call`
//! tear-off verdict, plus the structural-descent budget of the shared
//! assignability verdict.
//!
//! #3287 made the shared verdict right (`call_method_assignable`: solve the
//! generic `call` toward the context signature, else fall back to the
//! instantiate-to-bounds face, else reject a resolvable class with no
//! `call`), but only the right-hand-side positions reach it: the
//! `invalid_assignment` staging builds its `from` through
//! `rhs_expression_ty`, whose creation arm materializes the created
//! instance (`Ty::Named{C}`). An instance creation at an ARGUMENT position
//! (`take(C())`) never presents that instance face.
//!
//! Measured at this head (instrumented `read_type_at` + `assignable`
//! probes, since removed): the argument bare creation registers no
//! element at all — `flush_argument`'s window holds neither the creation
//! value row (it materializes in the send fold, after the argument list
//! closed) nor a typed head row, so `check_arguments`' per-argument walk
//! never asks the verdict. The two reporting pins below therefore stay
//! ignored until the argument window owns the creation's instance row
//! (`invocation.rs::flush_argument`, the live #3296 lane); the assignable
//! face is already correct for every position that does reach it, which
//! the silent and non-creation controls pin.
//!
//! Oracle behaviour pinned here (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/type_system.dart:875-884 +
//! `acceptsFunctionType`, :121-126):
//! - a callable class whose generic `call` solves to the context accepts
//!   at an argument position too (silent);
//! - a `call`-less class rejects, and the message names the class
//!   (ignored: the argument window never asks);
//! - a non-callable source (a `String`) rejects, and a send argument is
//!   walked at all;
//! - a matching plain `int` argument stays silent.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

use crate::FileReport;
use crate::diagnose::test_rig::report_ops;

/// The messages of one code.
fn rows<'a>(report: &'a FileReport, code: &str) -> Vec<&'a str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.as_str())
        .collect()
}

/// Every diagnostic name, in report order — the silence checks' own face.
fn codes(report: &FileReport) -> Vec<&str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// A generic `call` at an argument position instantiates toward the
/// formal and accepts — `take(C())` asks the context signature
/// `int Function(int)`, solves `T := int`, silent.
#[test]
fn callable_class_argument_solves_to_the_formal_context() {
    let fixture = "\
class C {
  T call<T>(T t) => t;
}

int take(int Function(int) f) => f(1);

void m() {
  take(C());
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "argument_type_not_assignable").is_empty(),
        "`take(C())` instantiates `call`'s T := int and accepts; got: {:?}",
        report.diagnostics
    );
}

/// `const` creations share the face — the keyword spelling is the same
/// instance creation upstream.
#[test]
fn const_creation_argument_solves_to_the_formal_context() {
    let fixture = "\
class C {
  const C();
  T call<T>(T t) => t;
}

int take(int Function(int) f) => f(1);

void m() {
  take(const C());
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "argument_type_not_assignable").is_empty(),
        "`take(const C())` instantiates and accepts; got: {:?}",
        report.diagnostics
    );
}

/// The written `<...>` group instantiates the created head before the
/// tear-off: `C<int>()` offers `C<int>`, whose `call` is already
/// concrete.
#[test]
fn instantiated_creation_argument_solves_to_the_formal_context() {
    let fixture = "\
class C<T> {
  T call(T t) => t;
}

int take(int Function(int) f) => f(1);

void m() {
  take(C<int>());
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "argument_type_not_assignable").is_empty(),
        "`take(C<int>())` offers `C<int>` and accepts; got: {:?}",
        report.diagnostics
    );
}

/// A named-selector creation reads the created instance too.
#[test]
fn named_creation_argument_solves_to_the_formal_context() {
    let fixture = "\
class C {
  C.named();
  T call<T>(T t) => t;
}

int take(int Function(int) f) => f(1);

void m() {
  take(C.named());
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "argument_type_not_assignable").is_empty(),
        "`take(C.named())` offers `C` and accepts; got: {:?}",
        report.diagnostics
    );
}

/// A `call`-less class never tears off — the argument rejects and the
/// message names the class, never an `Unknown`/`dynamic` residue.
///
/// Ignored for the same argument-window gate as
/// [`bounded_call_creation_argument_still_reports`] (the argument never
/// reaches the verdict walk, `invocation.rs::flush_argument`, #3296 lane).
#[ignore = "argument-window gate in invocation.rs::flush_argument (#3296 lane): a bare creation argument never reaches the verdict walk"]
#[test]
fn call_less_creation_argument_reports_the_class() {
    let fixture = "\
class D {}

void take(int Function(int) f) {}

void m() {
  take(D());
}
";
    let report = report_ops(fixture);
    let messages = rows(&report, "argument_type_not_assignable");
    assert_eq!(
        messages.len(),
        1,
        "one argument_type_not_assignable over the `call`-less creation; got: {:?}",
        report.diagnostics
    );
    assert!(
        messages[0].contains("'D'"),
        "the rejected argument names the created class; got: {}",
        messages[0]
    );
}

/// A non-callable source at the same argument position still reports —
/// the materialization arm must not blanket-accept (the control pin).
#[test]
fn non_callable_argument_still_reports() {
    let fixture = "\
void take(int Function(int) f) {}

void m() {
  take('x');
}
";
    let report = report_ops(fixture);
    let messages = rows(&report, "argument_type_not_assignable");
    assert_eq!(
        messages.len(),
        1,
        "one argument_type_not_assignable over the `String` argument; got: {:?}",
        report.diagnostics
    );
    assert!(
        messages[0].contains("'String'"),
        "the rejected argument names `String`; got: {}",
        messages[0]
    );
}

/// A matching non-callable argument stays silent (the second control:
/// `int` into an `int` formal — the argument face keeps its ordinary
/// matrix verdict).
#[test]
fn matching_primitive_argument_stays_silent() {
    let fixture = "\
void take(int x) {}

void m() {
  take(1);
}
";
    let report = report_ops(fixture);
    assert!(
        codes(&report).is_empty(),
        "`take(1)` on an `int` formal is silent; got: {:?}",
        report.diagnostics
    );
}

/// The control for the argument face itself: a SEND argument is checked —
/// this pin proves the per-argument walk reaches the shared verdict, so a
/// silent creation argument is the creation's own face, not the walk.
#[test]
fn send_argument_is_checked() {
    let fixture = "\
String g() => 'x';

void take(int Function(int) f) {}

void m() {
  take(g());
}
";
    let report = report_ops(fixture);
    let messages = rows(&report, "argument_type_not_assignable");
    assert_eq!(
        messages.len(),
        1,
        "one argument_type_not_assignable over the send argument; got: {:?}",
        report.diagnostics
    );
    assert!(
        messages[0].contains("'String'"),
        "the rejected send argument names `String`; got: {}",
        messages[0]
    );
}

/// The solved-but-mismatched creation still reports: the instantiation
/// answers `num Function(num)` over `T call<T extends num>(T)`, which no
/// `String Function(String)` formal accepts.
///
/// Ignored: measured at HEAD — the argument of a bare creation spelling
/// never reaches the per-argument verdict walk at all. `flush_argument`
/// registers no argument for `C()` (its row materializes only in the
/// send fold, after the argument list closed), so `check_arguments` never
/// asks and `read_type_at`'s creation face — which does answer
/// `C()`/`C<int>()`/`C.named()` — is never consulted. The gate lives in
/// `invocation.rs`'s argument window (`flush_argument`), owned by the
/// live #3296 lane; this pin flips back to `#[test]` once that window
/// registers the creation's instance row.
#[ignore = "argument-window gate in invocation.rs::flush_argument (#3296 lane): a bare creation argument never reaches the verdict walk"]
#[test]
fn bounded_call_creation_argument_still_reports() {
    let fixture = "\
class C {
  T call<T extends num>(T t) => t;
}

void take(String Function(String) f) {}

void m() {
  take(C());
}
";
    let report = report_ops(fixture);
    let messages = rows(&report, "argument_type_not_assignable");
    assert_eq!(
        messages.len(),
        1,
        "one argument_type_not_assignable over the bounded `call`; got: {:?}",
        report.diagnostics
    );
}

// ——— the structural-descent budget (#3333, folded into this pass) ———

/// A self-referential alias pair whose STRUCTURAL expansion re-enters the
/// shared verdict at every covariance level (`F` mentions `G`, `G` mentions
/// `F`, so `F <: G` asks `G <: F` asks `F <: G` …). The alias-name chain
/// is bounded, the structural descent is not: before the budget guard this
/// ask overflowed the analyzer thread's stack. The pin's assertion is
/// simply that the analysis returns — a stack overflow aborts the whole
/// binary, so reaching the assert IS the verdict — and that the verdict
/// stays in the under-report direction (an unknown answer fabricates no
/// rejection).
#[test]
fn self_referential_alias_pair_terminates() {
    let fixture = "\
typedef F = void Function(List<G>);
typedef G = void Function(List<F>);

void m(F f) {
  G g = f;
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "invalid_assignment").len() <= 1,
        "the capped descent under-reports rather than fabricating rows; got: {:?}",
        report.diagnostics
    );
}

/// The same pair across the CALL direction: the second edge of the cycle
/// (`fn_sig_subtype` → `slot_subtype` → `assignable`) is the one the
/// stack trace pinned, so the argument-position ask is pinned too.
#[test]
fn self_referential_alias_pair_argument_terminates() {
    let fixture = "\
typedef F = void Function(List<G>);
typedef G = void Function(List<F>);

void take(G g) {}

void m(F f) {
  take(f);
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "argument_type_not_assignable").len() <= 1,
        "the capped descent under-reports rather than fabricating rows; got: {:?}",
        report.diagnostics
    );
}

/// The multi-hop form of the same cycle: a three-alias ring
/// (`A → B → C → A`) keeps every single alias fold inside the name-hop
/// budget, so only the structural descent is unbounded — this is the shape
/// the #3333 stack trace's repeated frames came from (`assignable` →
/// `slot_subtype` → `covariant_args` → `fn_sig_subtype` → `assignable`).
#[test]
fn multi_hop_alias_ring_terminates() {
    let fixture = "\
typedef A = void Function(List<B>);
typedef B = void Function(List<C>);
typedef C = void Function(List<A>);

void m(A a) {
  C c = a;
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "invalid_assignment").len() <= 1,
        "the capped descent under-reports rather than fabricating rows; got: {:?}",
        report.diagnostics
    );
}

/// The ring across the argument direction too.
#[test]
fn multi_hop_alias_ring_argument_terminates() {
    let fixture = "\
typedef A = void Function(List<B>);
typedef B = void Function(List<C>);
typedef C = void Function(List<A>);

void take(C c) {}

void m(A a) {
  take(a);
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "argument_type_not_assignable").len() <= 1,
        "the capped descent under-reports rather than fabricating rows; got: {:?}",
        report.diagnostics
    );
}

// ——— the type-variable SOURCE arm (#3339 residual 2) ———

/// An UNBOUNDED in-scope `T` carries the implicit `Object?` upper bound, so
/// it fits the top targets and rejects every concrete non-nullable one —
/// upstream `subtype.dart:336-350` (Left Type Variable Bound) asked through
/// `isAssignableTo` (type_system.dart:860-891). Abstaining here was the
/// #3339 divergence on the corpus's largest live
/// `argument_type_not_assignable` mass.
#[test]
fn unbounded_type_parameter_argument_rejects() {
    let fixture = "\
void takeInt(int v) {}

void m<T>(T t) {
  takeInt(t);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable").len(),
        1,
        "one rejection over the unbounded `T`; got: {:?}",
        report.diagnostics
    );
}

/// The same pair at the assignment position — the abstention was
/// position-independent (`int x = t` stayed silent too).
#[test]
fn unbounded_type_parameter_initializer_rejects() {
    let fixture = "\
void m<T>(T t) {
  int x = t;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "invalid_assignment").len(),
        1,
        "one rejection over the unbounded `T`; got: {:?}",
        report.diagnostics
    );
}

/// The accept half: an unbounded `T` DOES fit the `Object?` target (the
/// implicit bound is the top-face object type) — the arm must not blanket
/// reject.
#[test]
fn unbounded_type_parameter_fits_object_question() {
    let fixture = "\
void m<T>(T t) {
  Object? o = t;
  dynamic d = t;
}
";
    let report = report_ops(fixture);
    assert!(
        rows(&report, "invalid_assignment").is_empty()
            && rows(&report, "argument_type_not_assignable").is_empty(),
        "`Object?`/`dynamic` accept an unbounded `T`; got: {:?}",
        report.diagnostics
    );
}

/// A BOUNDED parameter decides from its written bound: `T extends num`
/// fits `num` and rejects `String`.
#[test]
fn bounded_type_parameter_uses_its_bound() {
    let fixture = "\
void m<T extends num>(T t) {
  num n = t;
  String s = t;
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "invalid_assignment").len(),
        1,
        "only the `String` slot rejects the `num`-bounded `T`; got: {:?}",
        report.diagnostics
    );
}

// ——— the `Object?`-sourced argument (#3339 residual 1) ———

/// An `Object?` source into a non-nullable `Object` parameter reports; the
/// nullable parameter accepts the same source. The assignment position
/// already reports this pair (`assignable` answers `Some(false)`), so the
/// verdict is right and only the ARGUMENT READ can drop it.
///
/// Ignored: measured — the argument face yields nothing for
/// `takeObject(value)` while the assignment face reports the same pair, so
/// the missing piece is the argument's `from` (the `Object?`-typed read a
/// creation-free argument must present), i.e. `invocation.rs`'s argument
/// window, not this file's verdict. Flips back to `#[test]` with a verdict
/// change nowhere: the pair already decides.
#[ignore = "dormant: the `Object?`-sourced ARGUMENT read yields no verdict; the assignment face reports the same pair (invocation.rs argument window)"]
#[test]
fn nullable_source_argument_rejects() {
    let fixture = "\
void takeObject(Object o) {}
void takeObjectQuestion(Object? o) {}

void m(Object? value) {
  takeObjectQuestion('s');
  takeObject(value);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable").len(),
        1,
        "one rejection over the `Object` parameter; got: {:?}",
        report.diagnostics
    );
}
