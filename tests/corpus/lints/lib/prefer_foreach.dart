// prefer_foreach — official rule faces
// (pkg/linter prefer_foreach.dart:43-113): a for-each loop whose body is
// a single expression statement — through single-statement blocks and
// transparent parentheses — invoking a function with exactly one
// argument that is the loop variable itself reports at the whole for
// statement. A method invocation's target must not reference the loop
// variable; the identifier header form (`for (e in xs)`) never fires.
// Expected rows: 3 — PREFER_FOREACH keyed prefer_foreach.dart, message
// "Use 'forEach' and a tear-off rather than a 'for' loop to apply a
// function to every element.":
//   the `for (final a in list) { fn(a); }` block body
//   the `for (final a in list) fn(a);` non-block body
//   the `for (final element in list)` shadowing-closure target body
void f(List<int> list, void Function(int) fn) {
  for (final a in list) {
    fn(a);
  }
  for (final a in list) fn(a);
}

// GOOD: the loop variable as the invocation target keeps the rule silent.
class D {
  void handle(D d) {}
}

void g(List<D> list) {
  for (final d in list) {
    d.handle(d);
  }
}

// #1346 faces — the closure faces in the target (pkg/linter
// prefer_foreach.dart:76-84 with _ReferenceFinder :92-113): the target
// check reads resolution — a nested function literal whose own
// parameter (or a local it declares) rebinds the loop variable's name
// makes those lexemes the binder's, so the target does not reference
// the loop variable and the rule reports; a closure without such a
// binder captures the loop variable, and the reference keeps the rule
// silent.
class E {
  int f(E other) => 0;
}

void shadowingClosureFace(List<E> list) {
  for (final element in list) {
    list.map((element) => element).first.f(element);
  }
}

// GOOD: a closure without a binder of that name captures the loop
// variable, so the target references it and the rule stays silent.
void capturedClosureFace(List<E> list) {
  for (final element in list) {
    list.map(() => element).first.f(element);
  }
}
