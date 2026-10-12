// avoid_unused_constructor_parameters — official rule faces
// (pkg/linter avoid_unused_constructor_parameters.dart): a constructor
// parameter that is no field/super formal, carries no @deprecated, is not
// `_`-named, and is never referenced in the body or initializers reports
// at the parameter. External constructors skip.
// Expected rows: 2 — AVOID_UNUSED_CONSTRUCTOR_PARAMETERS keyed
// avoid_unused_constructor_parameters.dart, message
// "The parameter '{0}' is not used in the constructor.":
//   `int a` -> unreferenced positional parameter
//   `int c` -> unreferenced named parameter with a default
class C {
  int p;
  C(int a);
  C.named({int c = 0});
  C.used(int b) : p = b;
}

// GOOD: field formals, super formals, wildcards, deprecated metadata, and
// referenced parameters stay silent.
class D extends C {
  int q;
  D(this.q, int used) : super.named(c: used);
  D.wild(int _, int __);
  D.deprecated(@Deprecated('') int old);
}

// GOOD: an external constructor never reports.
class E {
  external E(int p);
}
