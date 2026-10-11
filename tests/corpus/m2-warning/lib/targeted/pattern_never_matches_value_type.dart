// Targets pattern_never_matches_value_type: no value of the matched enum can
// satisfy the unrelated class pattern (checkPatternNeverMatchesValueType).
enum E { a, b }

class A {}

void f(E e) {
  switch (e) {
    case A _:
      break;
    case E.a:
      break;
    case E.b:
      break;
  }
}
