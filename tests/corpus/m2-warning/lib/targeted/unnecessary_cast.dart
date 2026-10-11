// Targets unnecessary_cast: a cast whose operand is already of the cast type
// is dead (_isUnnecessaryCast, structural type equality; `x as num` over int
// stays a necessary downcast).
void f(int x) {
  var a = x as int;
  print(a);
}
