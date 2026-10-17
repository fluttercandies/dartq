// Trigger for dart_code_linter `avoid-passing-async-when-sync-expected`.
// Shape: an `async` closure/function passed where a synchronous
// `VoidCallback` is expected — the rule's flagged form.
void f() {
  run(onDone: () async {
    print('done');
  });
}

void run({required void Function() onDone}) {}
