// Trigger for solid_lints `no_empty_block`.
// Shape: `if`/`catch` blocks with no statements — the rule's flagged form.
void m(int value) {
  if (value > 0) {
  }

  try {
    value++;
  } catch (e) {
  }
}
