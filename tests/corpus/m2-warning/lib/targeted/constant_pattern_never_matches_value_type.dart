// Targets constant_pattern_never_matches_value_type: a constant pattern of a
// type that can never be equal to the matched value type (constant_verifier).
void f(int n) {
  switch (n) {
    case 'x':
      break;
  }
}
