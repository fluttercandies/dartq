// Targets return_of_invalid_type: a String is returned from a function whose
// return type is int (static nullability four, types-nullability-spec 3.3).
int f() {
  return 'not an int';
}
