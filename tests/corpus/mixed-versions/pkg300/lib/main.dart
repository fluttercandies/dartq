// Positive control at language version 3.0: records, patterns, and class
// modifiers (sealed/base/final/interface) all ship at 3.0 and are legal at
// the package default. Feature set derives from the library version
// (anchor: .third/dart-sdk/pkg/front_end/lib/src/source/
// source_compilation_unit.dart:348-353).
//
// Expectation: analyzes clean at 3.0.

(int, int, {String tag}) item = (1, 2, tag: 'pair');

sealed class Shape {}

base class Circle extends Shape {
  final double r;

  Circle(this.r);
}

final class Square extends Shape {
  final double side;

  Square(this.side);
}

interface class OpenBox {}

String label(Shape s) {
  return switch (s) {
    Circle(r: 0.0) => 'dot',
    Circle(r: var r) => 'circle $r',
    Square(side: var w) => 'square $w',
  };
}

void main() {
  print(label(Circle(1.0)));
  print(OpenBox());
}
