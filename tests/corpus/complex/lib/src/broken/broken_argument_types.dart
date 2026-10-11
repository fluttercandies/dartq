// Intentional diagnostics: the static-nullability three. Each failing line
// carries exactly one code: an argument mismatch at a call site, a direct
// assignment mismatch, and a return mismatch.
void recordWeight(int grams) {}

int thresholdFor(String unit) {
  // expect: argument_type_not_assignable
  recordWeight(unit);
  return unit.length;
}

void storeSizes() {
  // expect: invalid_assignment
  List<int> sizes = const <num>[1, 2, 3];
  print(sizes);
}

String labelFor(int code) {
  // expect: return_of_invalid_type
  return code;
}
