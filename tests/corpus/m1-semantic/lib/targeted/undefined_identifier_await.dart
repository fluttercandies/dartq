// Targets undefined_identifier_await: the bare name `await` is used as an
// ordinary identifier in a non-async function and resolves to nothing (the
// await-aware message variant of undefined_identifier).
void f() {
  print(await);
}
