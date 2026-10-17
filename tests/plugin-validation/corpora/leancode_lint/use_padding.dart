// Trigger for leancode_lint `use_padding`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/use_padding_test.dart):
// a `Container` that only sets `margin` — the dedicated `Padding` widget
// is preferred.
void test() {
  Container(
    margin: const EdgeInsets.all(10),
    child: const SizedBox(),
  );
}

class Container extends Widget {
  const Container({this.margin, this.child});

  final EdgeInsets? margin;
  final Widget? child;
}

class EdgeInsets {
  const EdgeInsets.all(double value);
}

class SizedBox extends Widget {
  const SizedBox();
}

class Widget {
  const Widget();
}
