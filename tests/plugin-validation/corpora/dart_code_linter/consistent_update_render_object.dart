// Trigger for dart_code_linter `consistent-update-render-object`.
// Shape: a `RenderObject`-backed widget whose `updateRenderObject` leaves a
// property unsynced — the rule's flagged form.
class MyWidget extends RenderObjectWidget {
  const MyWidget({required this.color});

  final Color color;

  @override
  RenderObject createRenderObject(BuildContext context) => MyRenderObject();

  @override
  void updateRenderObject(BuildContext context, MyRenderObject renderObject) {}
}

class MyRenderObject extends RenderObject {
  Color color = const Color(0);
}

class Color {
  const Color(int value);
}

class BuildContext {}

class RenderObject {}

abstract class RenderObjectWidget {
  const RenderObjectWidget();
}
