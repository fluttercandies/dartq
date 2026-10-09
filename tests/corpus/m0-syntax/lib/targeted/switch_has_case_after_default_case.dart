// Targets switch_has_case_after_default_case: the default case should be the
// last case in a switch statement.
void f(int x) {
  switch (x) {
    case 1:
      break;
    default:
      break;
    case 2:
      break;
  }
}
