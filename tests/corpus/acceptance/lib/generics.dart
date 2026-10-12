// Valid Dart 3.15 acceptance corpus — generics (generic classes and methods, extends-bounds, F-bounded parameters, nullable bounds, nested generic types, raw operator forms, instantiate-to-bound use).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/generic/, tests/language/generic_methods/, tests/language/instantiate_to_bound/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
class Box<T> {
  Box(this.value);

  T value;

  Box<R> map<R>(R Function(T) f) => Box(f(value));
}

class Cage<T extends Animal> {
  final List<T> occupants = <T>[];

  void add(T animal) => occupants.add(animal);

  T releaseFirst() => occupants.removeAt(0);
}

class SelfComparable<T extends Comparable<T>> {
  final T item;

  SelfComparable(this.item);

  bool atLeast(T other) => item.compareTo(other) >= 0;
}

class Pair<A, B extends num?> {
  const Pair(this.first, this.second);

  final A first;
  final B second;
}

class Nested {
  final Map<String, List<Box<int>>> data = <String, List<Box<int>>>{};

  List<Box<int>> operator [](String key) => data[key] ?? <Box<int>>[];
}

class Animal implements Comparable<Animal> {
  @override
  int compareTo(Animal other) => 0;
}

R pick<R>(R a, R b) => a;

T echo<T>(T value, {T Function(T)? transform}) =>
    transform == null ? value : transform(value);

void main() {
  final box = Box<int>(3);
  final mapped = box.map<String>((v) => 'v$v');
  final cage = Cage<Animal>();
  final animal = Animal();
  cage.add(animal);
  final released = cage.releaseFirst();
  final bounded = SelfComparable<Animal>(animal);
  final pair = Pair<String, int>('x', 4);
  const pair2 = Pair<String, int?>('y', null);
  final nested = Nested();
  nested['k'].add(box);
  final picked = pick<int>(1, 2);
  final echoed = echo(5, transform: (v) => v * 2);
  if (box.value +
          mapped.value.length +
          cage.occupants.length +
          released.compareTo(animal) +
          bounded.atLeast(animal).toString().length +
          pair.second +
          (pair2.second ?? 0) +
          nested['k'].length +
          picked +
          echoed !=
      18) {
    throw StateError('unreachable');
  }
}
