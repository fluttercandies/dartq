// Targets missing_statement: expected a statement (a reserved keyword in
// expression-statement position is recovered as an identifier).
void f() {
  class;
}
