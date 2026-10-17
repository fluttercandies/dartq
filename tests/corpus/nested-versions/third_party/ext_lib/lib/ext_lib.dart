// Vendored ext_lib 2.x at language version 3.0 (from `sdk: ^3.0.0`):
// records, sealed classes and pattern switch expressions are legal
// (3.0 positive control). The sibling major ext_lib_compat (2.19) pins
// the same-dependency-two-versions split — the two never mix in one
// package's dependency set.
//
// Expectation: analyzes clean at 3.0.

sealed class Shape {}

class Circle extends Shape {
  final double radius;

  Circle(this.radius);
}

class Square extends Shape {
  final double side;

  Square(this.side);
}

class ExtGreeter {
  static String greet(String who) => 'ext/$who';
}

String area(Shape shape) => switch (shape) {
      Circle(radius: final r) => 3.14 * r * r,
      Square(side: final s) => s * s,
    };

void main() {
  final pair = (area(Circle(2)), area(Square(3)));
  print(pair);
  print(ExtGreeter.greet('main'));
}
