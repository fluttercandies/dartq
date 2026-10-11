// Targets duplicate_hidden_name: one hide clause lists the same target-
// defined member twice; the repeat token reports (ImportsVerifier per-clause
// duplicate walk, WARNING tier). The use keeps the import used without
// touching the hidden names.
import 'dart:math' hide max, pi, max;

void main() {
  print(min(1, 2));
}
