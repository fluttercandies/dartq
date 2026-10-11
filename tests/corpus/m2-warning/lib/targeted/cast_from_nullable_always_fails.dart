// Targets cast_from_nullable_always_fails: a definitely-unassigned nullable
// local's read under a non-nullable cast target always throws
// (visitAsExpression reports at the operand identifier). The cast sits in an
// initializer — statement-position casts drop their leading operand in the
// parser's statement disambiguation.
void main() {
  int? x;
  var y = x as int;
  print(y);
}

// Negative: a non-nullable declared type fails the nullable-source gate, and
// `x as num` over int stays a necessary downcast, so no face reports.
void nonNullable(int x) {
  var y = x as num;
  print(y);
}
