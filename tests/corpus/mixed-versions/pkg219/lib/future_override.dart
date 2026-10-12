// @dart=3.0
// HIGHER-than-package override face: the package default is 2.19 but this
// file pins 3.0 (override semantics anchor:
// upstream Dart SDK source, pkg/front_end/lib/src/source/source_compilation_unit.dart
// :469-478).
//
// Records and patterns ship at 3.0, so these shapes are legal INSIDE this
// file while the rest of pkg219 stays at 2.19 — the mirror image of
// pkg300/lib/older_override.dart.
//
// Expectation: analyzes clean at the applied 3.0; the same shapes in
// pkg300/lib/older_override.dart (applied 2.19) ride the gate.

(int, int) pair = (1, 2);

sealed class Shape {}

class Circle extends Shape {
  final double r;

  Circle(this.r);
}

String area(Shape s) {
  return switch (s) {
    Circle(r: var radius) => '${radius * radius}',
  };
}

void main() {
  print(area(Circle(2.0)));
}
