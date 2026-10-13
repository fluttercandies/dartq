// Targets constant_pattern_with_non_constant_expression: a constant
// pattern whose expression is not a valid constant ('o++', the nail-pinned
// shape; an increment can never be const).
void f(int o) {
  switch (o) {
    case o++:
      break;
  }
}
