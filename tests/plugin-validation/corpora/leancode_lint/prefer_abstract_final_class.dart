// Trigger for leancode_lint `prefer_abstract_final_class`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/prefer_abstract_final_class_test.dart):
// a static-holder class that is neither abstract, final, nor sealed.
class MyConstants {
  MyConstants._();
  static const foo = 1;
}

class Helpers {
  Helpers._();

  static int compute(int value) => value * 2;
}
