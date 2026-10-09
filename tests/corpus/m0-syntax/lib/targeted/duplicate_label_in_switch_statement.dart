// Targets duplicate_label_in_switch_statement: the label was already used in
// this switch statement.
void f(int x) {
  switch (x) {
    L:
    case 1:
      break;
    L:
    case 2:
      break;
  }
}
