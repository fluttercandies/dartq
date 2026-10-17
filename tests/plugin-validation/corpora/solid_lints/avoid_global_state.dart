// Trigger for solid_lints `avoid_global_state`.
// Shape: mutable top-level state — a non-final global variable written from
// functions.
int counter = 0;

void bump() {
  counter++;
}
