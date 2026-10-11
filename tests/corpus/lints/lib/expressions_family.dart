// expression-statement family — fixture wave 2 kitchen file. Literal or
// written-type receivers only: the opaque-receiver presumption faces
// (the ledgered over-report family) stay out of shape, matching the
// wave-1 prefer_contains precedent.
// Expected rows: 7, all keyed expressions_family.dart —
// PREFER_CONDITIONAL_ASSIGNMENT (the brace-less single-statement if; the
// curly_braces single-line no-else exemption keeps it silent),
// PREFER_IF_NULL_OPERATORS (`label == null ? 'none' : label`),
// PREFER_NULL_AWARE_OPERATORS (`user == null ? null : user.handle` —
// peeling the access reaches the operand), PREFER_IS_NOT_OPERATOR
// (`!(candidate is int)` — the parenthesized is under a ! prefix),
// UNNECESSARY_NULL_IN_IF_NULL_OPERATORS (`maybe ?? null`),
// AVOID_FUNCTION_LITERALS_IN_FOREACH_CALLS (list-literal receiver, one
// closure parameter — the Map.forEach two-parameter shape stays out),
// NULL_CLOSURES (the named `orElse: null` face over a written List<int>
// receiver).
class Profile {
  String handle = '';
}

void labelUser(String? label) {
  if (label == null) label = 'none';
}

void mergeUser(String? label) {
  var merged = label == null ? 'none' : label;
  print(merged);
}

void pickUser(Profile? user) {
  var picked = user == null ? null : user.handle;
  print(picked);
}

void checkUser(Object candidate) {
  if (!(candidate is int)) {
    print(0);
  }
}

void nullUser(String? maybe) {
  var value = maybe ?? null;
  print(value);
}

void forEachUser() {
  <int>[1, 2].forEach((entry) {
    print(entry);
  });
}

void closureUser(List<int> xs) {
  xs.firstWhere((entry) => entry.isEven, orElse: null);
}
