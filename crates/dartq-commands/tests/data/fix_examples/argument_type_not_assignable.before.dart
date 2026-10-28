class A {
  int b = 1;
}
void g(int x) {}
void f(A? a) {
  g(a?.b);
}
