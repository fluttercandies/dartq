// Targets undefined_prefixed_name through an invocation position: a prefix
// applied to a function-shaped name the prefixed library does not declare
// (the getter-position shape lives in undefined_prefixed_name.dart).
// Trigger conditions: docs/facts/resolution-design-spec.md 4.2.
// Hand-crafted from the code semantics; no official test file.
import 'dart:math' as p;

void main() {
  p.noSuchFunction();
}
