// Trigger for dart_code_linter `prefer-media-query-direct-access`.
// Shape: `MediaQuery.of(context).size`-style whole-object access where a
// dedicated `MediaQuery.sizeOf` is preferred — the rule's flagged form.
void f(BuildContext context) {
  final size = MediaQuery.of(context).size;
}

class BuildContext {}

class MediaQuery {
  static MediaQueryData of(BuildContext context) => const MediaQueryData();
}

class MediaQueryData {
  const MediaQueryData();

  final size = const Size(0, 0);
}

class Size {
  const Size(double w, double h);
}
