// Trigger for dart_code_linter
// `check-for-equals-in-render-object-setters`.
// Shape: a `RenderObject` setter that assigns unconditionally without an
// `==` guard — the rule's flagged form.
class MyRenderObject extends RenderObject {
  Color _color = const Color(0);

  set color(Color value) {
    _color = value;
    markNeedsPaint();
  }

  void markNeedsPaint() {}
}

class RenderObject {
  void markNeedsPaint() {}
}

class Color {
  const Color(int value);
}
