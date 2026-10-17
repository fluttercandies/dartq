// Trigger for dart_code_linter `avoid-creating-vector-in-update`.
// Shape: a `Vector2` allocated inside the game-loop `update` method — the
// rule's flagged form.
class MyComponent extends Component {
  @override
  void update(double dt) {
    final newVector = Vector2(10, 10);
  }
}

class Component {
  void update(double dt) {}
}

class Vector2 {
  const Vector2(this.x, this.y);

  final double x;
  final double y;
}
