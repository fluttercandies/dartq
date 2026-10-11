// Targets cast_from_null_always_fails: a Null-typed expression under a
// non-nullable cast target always throws (visitAsExpression). The cast
// sits in an initializer — statement-position casts drop their leading
// operand in the parser's statement disambiguation.
void f(Null n) {
  var x = n as int;
  print(x);
}
