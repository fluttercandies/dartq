// Positive control at language version 3.7: non-binding wildcard
// formals. Both `_` parameters in dropAll are wildcards at 3.7 — no
// duplicate-definition error, nothing bound. The same shape in
// packages/core (3.6) is the below-version error face
// (lib/wildcard_repeat.dart), and in third_party packages at 3.0/2.19
// likewise.
//
// Expectation: analyzes clean at 3.7.

int sumIgnoring(int _, int y) => y;

int dropAll(int _, int _) => 0;

void main() {
  print(sumIgnoring(1, 2));
  print(dropAll(3, 4));
}
