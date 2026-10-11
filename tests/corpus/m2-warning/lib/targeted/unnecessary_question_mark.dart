// Targets unnecessary_question_mark: 'Null?' and 'dynamic?' gain nothing
// from the '?' (visitNamedType).
void f() {
  dynamic? a;
  Null? b;
  print(a);
  print(b);
}
