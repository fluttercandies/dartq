// Trigger for dart_code_linter `prefer-const-border-radius`.
// Shape: a `BorderRadius.circular` constructor invoked non-`const` — the
// rule's flagged form.
final radius = BorderRadius.circular(8);

class BorderRadius {
  const BorderRadius.circular(double radius);
}
