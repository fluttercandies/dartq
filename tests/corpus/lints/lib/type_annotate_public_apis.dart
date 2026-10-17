// type_annotate_public_apis — official rule faces
// (pkg/linter type_annotate_public_apis.dart:52-167): missing written
// types on public API declarations — a public method/getter or top-level
// function without a return type reports at its name; a public field or
// top-level variable with no written type reports per variable name
// (const and final with a non-null initializer exempt); a public legacy
// typedef without a return type reports at its name; untyped parameters
// of public faces report per name. Constructors skip enums and private
// owners or names.
// Expected rows: 4 — TYPE_ANNOTATE_PUBLIC_APIS keyed
// type_annotate_public_apis.dart, message
// "Missing type annotation on a public API.":
//   the legacy typedef name `F`
//   the untyped public top-level variable `v`
//   the untyped method parameter `p` of `A.m`
//   the untyped constructor parameter `q` of `A`
typedef F(x);

var v = 0;

class A {
  int m(p) => 1;

  A(q);
}

// GOOD: const top-level, typed members, and a private constructor pass.
const c = '';

class B {
  int n(int p) => 1;

  B(int q);

  B._(r);
}

class _Priv {
  _Priv(r);
}
