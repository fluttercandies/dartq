// use_primary_constructors — official rule faces
// (pkg/linter use_primary_constructors.dart:58-103): a class or enum
// without any constructor reports at the container name (the default
// constructor is the conversion target); exactly one non-redirecting
// generative constructor reports at it — the name part when written, else
// the type-name echo. A written primary constructor, an `external`
// constructor, and more than one generative constructor skip.
// Expected rows: 2 — USE_PRIMARY_CONSTRUCTORS keyed
// use_primary_constructors.dart, message "Use a primary constructor.":
//   the echo `C` of the lone unnamed constructor
//   the name `E` of the constructor-less enum
class C {
  C();
}

enum E {
  a, b, c
}

// GOOD: a written primary constructor and a two-constructor class skip.
class P();

class Q {
  Q();
  Q.named();
}
