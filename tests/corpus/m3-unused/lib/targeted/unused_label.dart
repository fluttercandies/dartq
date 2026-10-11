// Targets unused_label: a declared label that no break or continue names —
// the bare break exits the loop without referencing it.
void f() {
  loop:
  while (true) {
    break;
  }
}
