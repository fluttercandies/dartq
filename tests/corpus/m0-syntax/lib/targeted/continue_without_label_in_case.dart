// Targets continue_without_label_in_case: a continue statement in a switch
// statement must have a label as a target.
void f(int x) {
  switch (x) {
    case 1:
      continue;
  }
}
