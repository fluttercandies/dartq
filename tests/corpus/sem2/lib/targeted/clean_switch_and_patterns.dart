// Clean face for the patterns/switch family (non_exhaustive_switch_expression,
// constant_pattern_with_non_constant_expression, non_constant_case_expression):
// exhaustive switch expressions and constant patterns; both sides must
// stay silent.
enum E { a, b }

int f(E e) => switch (e) {
      E.a => 1,
      E.b => 2,
    };

void g(int o) {
  switch (o) {
    case 1:
      break;
    default:
      break;
  }
}

void main() {
  print(f(E.b));
  if (1 case final v) print(v);
}
