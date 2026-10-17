// Trigger for leancode_lint `never_discard_build_context`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/never_discard_build_context_test.dart):
// a `BuildContext` parameter named `_`.
Widget myBuilder(BuildContext _) {
  return const SizedBox();
}

class SizedBox extends Widget {
  const SizedBox();
}

class Widget {
  const Widget();
}

class BuildContext {}
