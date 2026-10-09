// Targets undefined_named_parameter: a call passes a named argument the
// callee does not declare (frozen M1 undefined family).
void f(int a) {}

void main() {
  f(b: 1);
}
