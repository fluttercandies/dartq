// Targets catch_syntax: 'catch' must be followed by '(identifier)' or
// '(identifier, identifier)'.
void f() {
  try {} catch () {}
}
