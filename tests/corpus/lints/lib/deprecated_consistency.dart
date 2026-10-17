// deprecated_consistency — official rule faces
// (upstream Dart SDK source,
// pkg/linter/lib/src/rules/deprecated_consistency.dart): deprecation must
// agree across related declarations. Three faces under one name: a
// constructor of a @deprecated class without its own annotation reports
// at the class-name echo; a `this.x` initializing formal whose public
// field is deprecated and whose parameter is not reports at the parameter
// (and the reverse direction at the field). Private fields skip; super
// formals skip; primary constructors are out of the substrate. The
// deprecation lookup folds to the annotation's name identifier —
// `deprecated` the constant or `Deprecated` the class.
// Expected rows: 2, both keyed deprecated_consistency.dart —
//   DEPRECATED_CONSISTENCY_CONSTRUCTOR at C's unnamed constructor (the
//   class-name echo)
//   DEPRECATED_CONSISTENCY_PARAMETER at E's `this.x` formal (field
//   deprecated, parameter not)
// Silence: D's constructor carries its own @Deprecated('x').
@deprecated
class C {
  C();
}

@deprecated
class D {
  @Deprecated('x')
  D();
}

class E {
  @deprecated
  int x = 0;

  E(this.x);
}
