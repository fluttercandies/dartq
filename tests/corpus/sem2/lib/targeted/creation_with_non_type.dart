// Targets creation_with_non_type: a 'new' expression whose name resolves
// to a non-class declaration (the top-level function 'f').
void f() {}

void main() {
  new f();
}
