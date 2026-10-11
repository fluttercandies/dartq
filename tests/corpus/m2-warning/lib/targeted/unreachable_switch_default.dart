// Targets unreachable_switch_default: a default clause after the cases
// already cover the scrutinee is unreachable (constant_verifier: coverage
// decided before the default clause).
enum Color { red, green, blue }

void f(Color c) {
  switch (c) {
    case Color.red:
      break;
    case Color.green:
      break;
    case Color.blue:
      break;
    default:
      break;
  }
}
