// Trigger for dart_code_linter
// `avoid-non-exhaustive-switch-on-sealed-classes`.
// Shape: a `switch` over a sealed type using a `default`/wildcard fallback
// instead of covering all subtypes — the rule's flagged form.
sealed class Shape {}

class Circle extends Shape {}

class Square extends Shape {}

String describe(Shape shape) {
  switch (shape) {
    case Circle():
      return 'circle';
    default:
      return 'unknown';
  }
}
