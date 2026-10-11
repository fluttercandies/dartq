// Targets unused_catch_stack: a stack-trace parameter that the catch body
// never uses (the exception e is used, isolating the code).
void f() {
  try {
  } catch (e, s) {
    print(e);
  }
}
