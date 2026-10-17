// Trigger for solid_lints `use_nearest_context`.
// Shape: a `BuildContext` passed to a helper instead of using the nearest
// enclosing context — the rule's flagged form.
Widget build(BuildContext context) {
  return helper(context);
}

Widget helper(BuildContext context) => const SizedBox();

class BuildContext {}

class SizedBox extends Widget {
  const SizedBox();
}

class Widget {
  const Widget();
}
