// Targets invalid_await_in_for: the keyword 'await' isn't allowed for a
// normal 'for' statement.
void f() async {
  await for (var i = 0; i < 1; i++) {}
}
