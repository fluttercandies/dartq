// Targets duplicate_shown_name: one show clause lists the same target-defined
// member twice; the repeat token reports (ImportsVerifier per-clause
// duplicate walk, WARNING tier). Every shown name is used, so no stray
// unused-shown-name hint rides along.
import 'dart:math' show max, min, max;

void main() {
  print(max(1, 2));
  print(min(1, 2));
}
