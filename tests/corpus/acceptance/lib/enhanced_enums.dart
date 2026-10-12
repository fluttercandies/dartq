// Valid Dart 3.15 acceptance corpus — enhanced enums (generative/const/factory enum constructors, fields, mixins/implements, generics with bounds, static members, getters, toString override).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/enum/enhanced_enums_basic_test.dart, tests/language/enum/syntax_test.dart (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
enum Color { red, green, blue }

enum Planet<T extends num> implements Comparable<Planet<T>> {
  mercury(3.303e23, 2.4397),
  venus(4.869e24, 6.0518);

  const Planet(this.mass, this.radius);

  final T mass;
  final T radius;

  @override
  int compareTo(Planet<T> other) => 0;
}

enum Vehicle with Wheels implements Describable {
  car(wheels: 4, doors: 4),
  bike(wheels: 2, doors: 0);

  const Vehicle({required this.wheels, required this.doors});

  final int wheels;
  final int doors;

  @override
  String describe() => '$name has $wheels wheels';
}

enum Singly {
  a,
  b;

  static const lowest = Singly.a;

  int get indexShifted => index + 1;

  @override
  String toString() => 'Singly.$name';
}

mixin Wheels {
  int get wheels => 4;
}

abstract interface class Describable {
  String describe();
}

void main() {
  final all = <Object>[
    Color.values,
    Color.red.index,
    Color.green.name,
    Color.blue.hashCode,
    Planet.mercury.mass,
    Planet.venus.radius,
    Planet.mercury.compareTo(Planet.venus),
    Vehicle.car.describe(),
    Vehicle.bike.doors,
    Vehicle.values.length,
    Vehicle.car.wheels,
    Singly.b.indexShifted,
    Singly.lowest,
    Singly.a.toString(),
  ];
  if (all.length != 14) {
    throw StateError('unreachable');
  }
}
