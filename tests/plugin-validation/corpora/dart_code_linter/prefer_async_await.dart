// Trigger for dart_code_linter `prefer-async-await`.
// Shape: `Future.then` used where `await` reads better — the rule's
// flagged form.
void f(Future<String> later) {
  later.then((value) => print(value));
}
