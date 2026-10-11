// Targets unreachable_switch_case: a case covered by the preceding cases is
// unreachable (constant_verifier's per-case reachability pass).
enum Color { red, green, blue }

void f(Color c) {
  switch (c) {
    case Color.red:
      break;
    case Color.green:
      break;
    case Color.blue:
      break;
    case Color.red:
      break;
  }
}
