// exhaustive_cases — official rule faces (pkg/linter exhaustive_cases.dart
// + asEnumLikeType in extensions.dart): a switch over an enum-like type —
// a concrete class with only private generative constructors and two or
// more static const fields of its own type — must cover every constant
// value or carry a default. Real enums never enter the rule (the analyzer's
// own exhaustiveness checking covers them), so the faces here are
// enum-like classes, per the official test_class_enumLike_* data. The
// constants carry distinct field values on purpose: constant-value identity
// (computeConstantValue) groups aliased initializers, so equal-valued
// constants would collapse into one key and covering one would cover both.
// Expected rows: 1 — EXHAUSTIVE_CASES keyed exhaustive_cases.dart:
//   the switch in `f` covers `E.e` but omits the value of `E.f`
//   -> row at the switch head.
class E {
  final int i;

  const E._(this.i);

  static const e = E._(1);
  static const f = E._(2);
}

void f(E e) {
  switch (e) {
    case E.e:
      break;
  }
}

// GOOD: every constant value covered — silent.
void ok(E e) {
  switch (e) {
    case E.e:
      break;
    case E.f:
      break;
  }
}

// GOOD: a default clause silences the check.
void withDefault(E e) {
  switch (e) {
    case E.e:
      break;
    default:
      break;
  }
}
