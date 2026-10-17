// Trigger for solid_lints `prefer_early_return`.
// Shape: a function whose result is only produced at the bottom of nested
// conditionals instead of returning early.
int? classify(int? value) {
  int? result;

  if (value != null) {
    if (value > 0) {
      result = 1;
    } else {
      result = -1;
    }
  }

  return result;
}
