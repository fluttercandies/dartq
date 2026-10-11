// Targets duplicate_import: the same library imported twice by one unit with
// identical directive shapes reports at the repeat's URI (ImportsVerifier
// duplicate pair walk, WARNING tier).
import 'dart:math';

import 'dart:math';

void main() {
  print(max(1, 2));
}
