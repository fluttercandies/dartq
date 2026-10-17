// Trigger for leancode_lint `use_align`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/use_align_test.dart):
// a `Container` that only sets `alignment` — the dedicated `Align` widget
// is preferred.
void test() {
  Container(
    alignment: Alignment.center,
    child: const SizedBox(),
  );
}

class Container extends Widget {
  const Container({this.alignment, this.child});

  final Alignment? alignment;
  final Widget? child;
}

class Alignment {
  const Alignment._();
  static const center = Alignment._();
}

class SizedBox extends Widget {
  const SizedBox();
}

class Widget {
  const Widget();
}
