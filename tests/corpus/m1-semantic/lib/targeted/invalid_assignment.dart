// Targets invalid_assignment: a String value is assigned to an int variable
// (static nullability four, types-nullability-spec 3.1).
void main() {
  int x = 'not an int';
  print(x);
}
