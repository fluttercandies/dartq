// Trigger for leancode_lint `use_dedicated_media_query_methods`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/use_dedicated_media_query_methods_test.dart):
// `MediaQuery.of(context).size`-style access where a dedicated
// `MediaQuery.sizeOf` exists.
Widget build(BuildContext context) {
  final width = MediaQuery.of(context).size.width;
  return const SizedBox();
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
  const Size(this.width, this.height);
  final double width;
  final double height;
}

class SizedBox extends Widget {
  const SizedBox();
}

class Widget {
  const Widget();
}
