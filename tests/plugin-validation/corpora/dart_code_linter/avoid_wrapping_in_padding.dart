// Trigger for dart_code_linter `avoid-wrapping-in-padding`.
// Shape: a `Container` with `padding` wrapped in a `Padding` widget — the
// rule's flagged form.
final widget = Padding(
  padding: const EdgeInsets.all(8),
  child: Container(padding: const EdgeInsets.all(4)),
);

class Padding extends Widget {
  const Padding({required this.padding, this.child});

  final EdgeInsets padding;
  final Widget? child;
}

class Container extends Widget {
  const Container({this.padding});

  final EdgeInsets? padding;
}

class EdgeInsets {
  const EdgeInsets.all(double value);
}

class Widget {
  const Widget();
}
