// Trigger for dart_code_linter `prefer-correct-edge-insets-constructor`.
// Shape: `EdgeInsets.fromLTRB` with symmetric values that a simpler
// `EdgeInsets.all`/`symmetric` covers — the rule's flagged form.
final padding = EdgeInsets.fromLTRB(8, 8, 8, 8);

class EdgeInsets {
  const EdgeInsets.fromLTRB(double l, double t, double r, double b);
}
