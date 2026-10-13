// Targets not_enough_positional_arguments: a call passing fewer positional
// arguments than the function declares.
void g(int a) {}

void main() {
  g();
}
