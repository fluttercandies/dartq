//! The #3477 pins — extension members get no use-credit on the shapes the
//! resolve side resolves but the finalize arm never records.
//!
//! `credit_derived_extension_uses`'s byte-scan arms (the #3223/#3289
//! censuses) credit the literal/`Function`/`Type` receiver heads only:
//! every other resolved application — a generic function-`on` compound
//! (`f += 1` over `extension on void Function<T>(T)`), a record-`on`
//! operator or `call` sugar (`extension on (String,)` + `a + 0` / `a()`),
//! a prefix operator (`~"hello"` over `extension _A on String`), an
//! explicit-`this` `??=` write setter (`this.p ??= v` inside `extension
//! on String`) — resolved through `extension_application`/`record_
//! extension_face` at pass 2 but never joined `extension_member_used`, so
//! the unnamed/private face loop reported the member as unused.
//!
//! The credit now re-derives from the row pool's own intents: an
//! `Intent::Operator` row's receiver type answers the application (named
//! heads through the face lattice, record and function rows through the
//! shape-extension faces), a `ReadWrite` row's write side credits the
//! face's `name=` setter the same way `accept_setter` already does
//! resolve-side, and a bare `Invoke` over a callee the locals table types
//! record/function folds `call`. Every pin keeps the guard chain the
//! existing arms run — exactly one accessible face declares the spelling
//! and the face's `on` shape admits the receiver.
//!
//! Oracle provenance: `dart analyze` reports no `unused_element` on any of
//! the fixtures below (the discriminating conformance rows
//! `test_assignmentExpression_compound#0`, `test_expression_recordType_
//! hasCall_extensionMethod#0`, and the
//! `if_null_assignment_implicit_extension_*` family).
//!
//! The extension-body sweep adds the shapes no row record carries at all:
//! a bare implicit-`this` `??=`/compound write inside the face's own body
//! (the write half produces no read-position ref) and a `this(...)`
//! invocation (`RefTarget::This` rides no application arm) — both credit
//! the enclosing face directly. The literal arm also skips a null
//! assertion between the literal and its application (`42!(2)`).
//! Routed out: the `on void` / `on Record` supertype admissions — the
//! resolve side rejects the read itself (`undefined_getter`), so the
//! finalize arm never sees the row; the owning layer is
//! `extension_application`'s admission.
//!
//! Registered here via `#[path]`; the module stays inside the `diagnose`
//! subtree so `crate::diagnose::test_rig` resolves.

use super::FileReport;
use super::test_rig::{report_ops, rows};

/// The `unused_element`/`unused_field` rows of `report`, rendered for the
/// failure message (the family's lens — every fixture must be silent).
fn unused(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element" || d.code.name == "unused_field")
        .map(|d| format!("{}@{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

fn assert_silent(fixture: &str) {
    let report = report_ops(fixture);
    let bad = unused(&report);
    assert!(bad.is_empty(), "{bad:?}\n{}", rows(&report).join("\n"));
}

// —— operators over a generic function `on` face ——————————————————————————

/// `test_assignmentExpression_compound`: `f += 1` over `extension on void
/// Function<T>(T)` credits `operator+` — the compound write resolves the
/// operator through the face, the member stays used.
#[test]
fn generic_function_on_compound_credits_operator() {
    let fixture = "\
extension on void Function<T>(T) {
  void Function<T>(T) operator +(int i) {
    return this;
  }
}
void Function(int) foo(void Function<T>(T) f) {
  return f += 1;
}
";
    assert_silent(fixture);
}

/// `test_postfixExpression_compound`'s twin: `f++` resolves the same
/// `operator+` through the function-`on` face.
#[test]
fn generic_function_on_postfix_credits_operator() {
    let fixture = "\
extension on void Function<T>(T) {
  void Function<T>(T) operator +(int i) {
    return this;
  }
}
void foo(void Function<T>(T) f) {
  f++;
}
";
    assert_silent(fixture);
}

/// The unnamed generic `on T Function(T)` face: `g *= (i) => i + 10` over
/// `extension<T> on T Function(T)` credits `operator*`.
#[test]
fn generic_fn_typevar_on_compound_credits_operator() {
    let fixture = "\
extension<T> on T Function(T) {
  T Function(T) operator*(T Function(T) other) {
    return (value) => this(other(value));
  }
}
void f() {
  var g = (int i) => i + 1;
  g *= (i) => i + 10;
}
";
    assert_silent(fixture);
}

// —— record `on` faces ————————————————————————————————————————————————————

/// `test_expression_recordType_hasCall_extensionMethod`: `a()` over
/// `extension on (String,)` is `call` sugar — the face's `call` stays used.
#[test]
fn record_on_call_sugar_credits_call() {
    let fixture = "\
void f((String,) a) {
  a();
}
extension on (String,) {
  int call() => 0;
}
";
    assert_silent(fixture);
}

/// `test_expression_recordType_hasOperator`: `a + 0` over `extension on
/// (String,)` credits `operator+`.
#[test]
fn record_on_operator_credits_operator() {
    let fixture = "\
void f((String,) a) {
  a + 0;
}
extension on (String,) {
  int operator +(int other) => 0;
}
";
    assert_silent(fixture);
}

// —— prefix operators ————————————————————————————————————————————————————

/// `test_method_isUsed_privateExtension_prefixOperator`: `~"hello"` over
/// `extension _A on String` credits `operator~`.
#[test]
fn private_extension_prefix_operator_credits() {
    let fixture = "\
extension _A on String {
  int operator ~() => 7;
}
void main() {
  ~'hello';
}
";
    assert_silent(fixture);
}

// —— the `??=` write setter ——————————————————————————————————————————————

/// `if_null_assignment_implicit_extension_*`: `this.p ??= v` inside
/// `extension on String` credits BOTH the `p` getter (the read half) and
/// the `p=` setter (the write half) — a bare `p ??= v` resolves the same
/// implicit-`this` application.
#[test]
fn this_property_null_assign_credits_getter_and_setter() {
    let fixture = "\
extension on String {
  int? get pIntQuestion => null;
  set pIntQuestion(Object? value) {}
}
void f() {
  '' .pIntQuestion ??= 1;
}
";
    assert_silent(fixture);
}

// —— the extension-body sweep (implicit-`this` writes and invocations) ———

/// The `if_null_assignment_implicit_extension_implicit_this_property_test`
/// discriminator: `p ??= v` on a BARE name inside the face's own body —
/// the write half produces no read-position record, so the sweep credits
/// the accessor pair from the token stream. Every accessor pair of the
/// census fixture's face stays used.
#[test]
fn implicit_this_if_null_write_credits_getter_and_setter() {
    let fixture = "\
class A {}
class B1<T> implements A {}
class B2<T> implements A {}
class C1<T> implements B1<T>, B2<T> {}
extension on String {
  double? get pDoubleQuestion => null;
  set pDoubleQuestion(Object? value) {}
  int? get pIntQuestion => null;
  set pIntQuestion(Object? value) {}
  String get pString => '';
  set pString(Object? value) {}
  String? get pStringQuestion => null;
  set pStringQuestion(String? value) {}
  test() {
    pString ??= contextType('');
    pStringQuestion ??= contextType('');
    var d = 2.0;
    context<Object>((pIntQuestion ??= d));
    context<Object>((pDoubleQuestion ??= d));
  }
}
Object? contextType(Object? x) => x;
void context<T>(Object? x) {}
void main() {
  ''.test();
}
";
    assert_silent(fixture);
}

/// `Extension-methods/call_member_t10`: `this(42)` inside the face's own
/// body is the implicit-`call` invocation on a value of the `on` type —
/// the face's `call` stays used.
#[test]
fn this_invocation_credits_call() {
    let fixture = "\
class C {}
extension on C {
  test() {
    Expect.equals('Lily was here: 42', this(42));
  }
  String call(int v) => 'Lily was here: $v';
}
class Expect {
  static void equals(expected, actual) {}
}
void main() {
  C c = new C();
  c.test();
}
";
    assert_silent(fixture);
}

/// `nnbd/null_check_operator_A05_t02`: a null assertion between the
/// literal and its application (`42!(2)`, `42![24]`, `42![24] = 24`) —
/// `e!` keeps the literal's own head, so `call`, `[]`, and `[]=` all
/// stay used.
#[test]
fn null_asserted_literal_application_credits_call_and_index() {
    let fixture = "\
extension on int {
  String call(int i) => 'Lily was here $i times';
  Object? operator [](int index) => index;
  void operator []=(int index, int val) {}
}
void main() {
  42!(2);
  42![24];
  42![24] = 24;
}
";
    assert_silent(fixture);
}
