// Targets the static-nullability trio in one assignment-flow file:
// invalid_assignment (a String value assigned to an int variable),
// argument_type_not_assignable (a String argument where int is expected)
// and return_of_invalid_type (an int expression returned from a String
// function, expression-body form). Trigger conditions: the pinned
// nullability table (NNBD assignability and return-type checks). Hand-crafted from the code semantics;
// no official test file.
void take(int x) {}

String give() => 1;

void main() {
  int value = 'not an int';
  take('not an int');
  print(value);
  print(give);
}
