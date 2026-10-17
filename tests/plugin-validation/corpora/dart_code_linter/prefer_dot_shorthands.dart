// Trigger for dart_code_linter `prefer-dot-shorthands`.
// Shape: a static member accessed through its full type name where a dot
// shorthand is available — the rule's flagged form.
Color color = Color.red;

class Color {
  static const red = Color();

  const Color();
}
