// Valid Dart 3.15 acceptance corpus — class modifiers (sealed, base, final,
// interface, abstract, mixin class, base mixin; in-library extension and
// implementation; sealed-hierarchy exhaustive switch expression).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/class_modifiers/ and
// upstream Dart SDK source, tests/language/sealed_class/ (SDK main @ e6f4207a).
// Covered by scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check`
// machine face).

sealed class Shape {
  const Shape();
}

final class Circle extends Shape {
  final double radius;
  const Circle(this.radius);
}

final class Square extends Shape {
  final double side;
  const Square(this.side);
}

base class Vehicle {
  void drive() {}
}

base class Car extends Vehicle {}

final class Locked {}

final class LockedSub extends Locked {}

interface class Contract {
  void act() {}
}

class ContractImpl implements Contract {
  @override
  void act() {}
}

mixin class Reactive {
  void react() {}
}

class User with Reactive {}

abstract base class Storage {
  int size();
}

class Memory extends Storage {
  @override
  int size() => 1;
}

base mixin Logging {
  void log(String message) {}
}

class Logged with Logging {}

String label(Shape shape) => switch (shape) {
      Circle(:radius) => 'circle $radius',
      Square(:side) => 'square $side',
    };

void main() {
  final shapes = <Shape>[const Circle(1), const Square(2)];
  final labels = [for (final s in shapes) label(s)];
  Car().drive();
  LockedSub();
  ContractImpl().act();
  User().react();
  Logged().log('hello');
  final capacity = Memory().size();
  if (labels.length != 2 || capacity != 1) {
    throw StateError('unreachable');
  }
}
