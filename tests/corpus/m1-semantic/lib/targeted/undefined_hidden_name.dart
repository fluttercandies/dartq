// Targets undefined_hidden_name: a hide combinator names a member the
// imported dart: library does not export.
import 'dart:math' hide noSuchName;

void main() {
  print(max(1, 2));
}
