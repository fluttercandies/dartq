// Targets expected_else_or_comma: expected 'else' or comma (collection-if
// element followed by another element without a comma).
void f(bool a) {
  var x = [if (a) 1 2];
}
