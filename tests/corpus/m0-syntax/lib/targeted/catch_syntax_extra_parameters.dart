// Targets catch_syntax_extra_parameters: 'catch' must be followed by
// '(identifier)' or '(identifier, identifier)'.
void f() {
  try {} catch (e, s, t) {}
}
