// Targets unnecessary_wildcard_pattern: a wildcard pattern as a logical-and
// operand always matches (shared_type_analyzer).
void f(Object o) {
  switch (o) {
    case _ && _:
      break;
  }
}
