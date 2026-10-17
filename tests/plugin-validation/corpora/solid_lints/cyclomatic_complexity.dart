// Trigger for solid_lints `cyclomatic_complexity`.
// Shape: a function whose decision count exceeds the configured
// complexity threshold — the rule's flagged form.
String f(int val) {
  if (val == 0) {
    return 'zero';
  } else if (val == 1) {
    return 'one';
  } else if (val == 2) {
    return 'two';
  } else if (val == 3) {
    return 'three';
  } else {
    return 'many';
  }
}
