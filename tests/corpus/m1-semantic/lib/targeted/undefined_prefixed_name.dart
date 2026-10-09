// Targets undefined_prefixed_name: an import prefix is applied to a name the
// prefixed library does not declare.
import 'dart:math' as p;

void main() {
  print(p.noSuchName);
}
