// type_literal_in_constant_pattern — audit r1 thin row, kept to the
// both-sides-agree face: a bare constant pattern whose lexeme is a
// built-in type name on a non-Type scrutinee (the dartq ported face is
// the built-in-name set, flow.rs type_literal_in_constant_pattern; the
// oracle's resolution face reports the same row here). User-type
// literals (`case MyType`) are the dartq under-report face and stay out;
// user constants named like built-ins are the over-report face and stay
// out.
// Expected rows: 1 — TYPE_LITERAL_IN_CONSTANT_PATTERN keyed
// type_literal_in_constant_pattern.dart (`case int` — did you mean
// `int _`).
void f(Object o) {
  if (o case int) {
    print(0);
  }
}
