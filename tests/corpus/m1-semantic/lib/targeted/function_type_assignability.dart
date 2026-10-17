// Targets argument_type_not_assignable (the F7 face: function-type
// assignability at call sites) on arguments that are closures or
// tear-offs checked against the parameter's declared function type:
// return-side covariance, formal-side contravariance, the int?/int
// null-safety pair in both directions, required-formal count, and the
// named-vs-positional / required-flag shapes, with the sound
// counterparts pinned as deliberate silences. Trigger conditions: the
// pinned assignability table (function-subtype relations on argument
// function types). Hand-crafted from the code semantics; no official
// test file.
//
// Upstream anchors (upstream Dart SDK source):
// - pkg/analyzer/lib/src/dart/element/subtype.dart: _isFunctionSubtypeOf
//   :86 — return covariance :98, formal contravariance :112 (the target
//   formal's type must be a subtype of the source formal's type), named
//   required-flag :143, named coverage :157, leftover required
//   positionals :168, leftover target formals :177; Right-Top (void as
//   the target return type accepts any return type) in _isSubtypeOf
//   :281-288.
// - pkg/analyzer/lib/src/dart/element/type_system.dart: isAssignableTo
//   :890 — with default strict-casts, non-dynamic downcasts are disabled
//   :917-924, so argument assignability reduces to subtyping here.
// - pkg/analyzer/lib/src/generated/error_detection_helpers.dart:
//   checkForArgumentTypeNotAssignableForArgument :67 gates each resolved
//   argument on isAssignableTo :110 and reports
//   diag.argumentTypeNotAssignable :426 (diagnostic name
//   argument_type_not_assignable, COMPILE_TIME_ERROR).
//
// Oracle rows: counts approximate — this header was authored from the
// upstream source, no dart run happened in the authoring lane; the
// serial N2 differential leg pins the exact rows (code, line, column,
// message). Expected: 7 argument_type_not_assignable rows at the call
// sites marked ERROR below; every site marked SILENT must produce no
// row.
void useIntParam(void Function(int) f) => f(1);

void useNumParam(void Function(num) f) => f(1.5);

void useNullableParam(void Function(int?) f) => f(null);

void useIntReturn(int Function() f) => print(f());

void useNullableIntReturn(int? Function() f) => print(f());

void useOneRequired(void Function(int) f) => f(1);

void useTwoRequired(void Function(int, int) f) => f(1, 2);

void useNamed(void Function({int x}) f) => f();

void useRequiredNamed(void Function({required int x}) f) => f(x: 1);

void useVoidReturn(void Function() f) => f();

int fromNum(num n) => n.toInt();

int justInt() => 1;

int? maybeInt() => null;

void onlyInt(int x) {}

void nullableOk(int? x) {}

void oneRequired(int a) {}

void twoRequired(int a, int b) {}

void positionalX(int x) {}

void requiredNamedX({required int x}) {}

void optionalNamedX({int x}) {}

void main() {
  // (a) Assignable controls: an exact-shape closure and a tear-off whose
  // formal widens int to num. Contravariance accepts the supertype
  // (subtype.dart:112, int <: num), return int <: void is Right-Top
  // (subtype.dart:281), so both are SILENT.
  useIntParam((int x) {});
  useIntParam(fromNum);

  // (b) Formal contravariance violation: the target accepts num, the
  // argument accepts only int; subtype.dart:112 needs num <: int and
  // fails, so this reports. The widening direction is the silent control
  // in (a): accepting more than demanded is sound and never reports.
  useNumParam(onlyInt); // ERROR argument_type_not_assignable

  // (g) int?/int null-safety pair, both directions: an argument accepting
  // only int where int? is demanded reports (int? <: int fails at
  // subtype.dart:112); an argument accepting int? where only int is
  // demanded is a sound upcast and stays SILENT.
  useNullableParam(onlyInt); // ERROR argument_type_not_assignable
  useIntParam(nullableOk); // SILENT

  // (c) Return covariance: the argument's return type must be a subtype
  // of the target's (subtype.dart:98). Returning int? where int is
  // demanded reports; returning int where int? is demanded is SILENT.
  useIntReturn(maybeInt); // ERROR argument_type_not_assignable
  useNullableIntReturn(justInt); // SILENT

  // (d) Required-formal count, both ways: a two-required-formal target
  // rejects a one-formal argument (leftover target formals,
  // subtype.dart:177) and a one-formal target rejects a
  // two-required-formal argument (leftover required positionals,
  // subtype.dart:168).
  useTwoRequired(oneRequired); // ERROR argument_type_not_assignable
  useOneRequired(twoRequired); // ERROR argument_type_not_assignable

  // (e) Named-shape mismatch: a positional argument where the target
  // declares a named formal reports (subtype.dart:118); a required-named
  // argument where the target's named formal is optional reports
  // (subtype.dart:143, callers may omit x). The reverse — an
  // optional-named argument where the target's formal is required — is
  // sound because the call site always passes x (subtype.dart:145) and
  // stays SILENT.
  useNamed(positionalX); // ERROR argument_type_not_assignable
  useNamed(requiredNamedX); // ERROR argument_type_not_assignable
  useRequiredNamed(optionalNamedX); // SILENT

  // (f) A void-return target accepts any return type: Right-Top makes
  // int <: void hold (subtype.dart:281-288), so this is a deliberate
  // SILENCE, not a missed check.
  useVoidReturn(justInt); // SILENT
}
