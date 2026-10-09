// Targets switch_has_multiple_default_cases: the 'default' case can only be
// declared once.
void f(int x) {
  switch (x) {
    case 1:
      break;
    default:
      break;
    default:
      break;
  }
}
