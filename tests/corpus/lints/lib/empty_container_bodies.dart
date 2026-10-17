// empty_container_bodies — official rule faces
// (pkg/linter empty_container_bodies.dart:27-45): an empty `{}` container
// body with only whitespace between the braces reports its brace pair
// with the container kind word. Any comment between the braces silences
// the rule; a member row is not an empty body; enum bodies are a
// different upstream face and never report here.
// Expected rows: 3 — EMPTY_CONTAINER_BODIES keyed
// empty_container_bodies.dart, messages
// "Empty class bodies should be written using a ';' rather than '{}'.",
// "Empty mixin bodies should be written using a ';' rather than '{}'.",
// "Empty extension bodies should be written using a ';' rather than '{}'.":
//   the `{}` of `class C`
//   the `{}` of `mixin M`
//   the `{}` of the unnamed extension
class C {
}

mixin M {}

extension on String {}

// GOOD: a member row and a commented body pass.
class D {
  D();
}

class E {
  // eol
}
