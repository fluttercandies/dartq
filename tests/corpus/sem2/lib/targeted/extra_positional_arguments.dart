// Targets extra_positional_arguments: a call passing more positional
// arguments than the function declares. Oracle message (nail-pinned):
// "Too many positional arguments: 0 expected, but 1 found."
void f() {}

void main() {
  f(1);
}
