// @dart = 3.12
// var_with_no_type_annotation — official rule faces
// (pkg/linter var_with_no_type_annotation.dart:34-66): a parameter whose
// storage keyword is `var` with no written type reports at the keyword —
// plain, named, and optional formals alike. Typed, unmarked, and `final`
// parameters pass; super formals and function-typed parameters skip (the
// compile-time-error families).
//
// The rule registers only WITHOUT primary constructors (the declaring
// form supersedes it), so this fixture pins the library to 3.12 — the
// pre-primary-constructors face — where both sides register it.
// Expected rows: 3 — VAR_WITH_NO_TYPE_ANNOTATION keyed
// var_with_no_type_annotation.dart, message
// "Avoid declaring parameters with `var` and no type annotation.":
//   the `var` of `x` in `f`
//   the `var` of the named formal `x` in `g`
//   the `var` of the optional formal `z` in `h`
void f(var x) {}

void g({var x = 1}) {}

void h([var z]) {}

// GOOD: typed, unmarked, and final parameters pass.
void ok(int a, b, final c) {}
