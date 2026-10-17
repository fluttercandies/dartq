// avoid_print — official rule faces
// (upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_print.dart):
// a MethodInvocation whose callee resolves to dart:core's `print` reports
// at the callee, and every argument-position `print` SimpleIdentifier
// that resolves to dart:core's `print` reports at the identifier — both
// faces ride the one MethodInvocation visitor. A resolvable same-file
// local declaration shadows the core function and stays silent, and a
// bare non-argument reference is never visited.
// Expected rows: 3 AVOID_PRINT keyed avoid_print.dart —
//   the `print('x')` call in a
//   the `xs.forEach(print)` tear-off argument in b (at the identifier)
//   the `c(print)` plain argument in d (at the identifier)
// Silences: e's local `print` shadow; f's bare `print;` reference.
// UNNECESSARY_STATEMENTS (options standalone section) co-fires at f's
// `print;` — a symmetric both-sides row.
void a() {
  print('x');
}

void b(List<int> xs) {
  xs.forEach(print);
}

void c(int x) {}

void d() {
  c(print);
}

void e() {
  void print(Object? o) {}

  print('local');
}

void f() {
  print;
}
