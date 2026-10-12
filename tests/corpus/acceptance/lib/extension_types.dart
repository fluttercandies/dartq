// Valid Dart 3.15 acceptance corpus — extension types (representation declarations, const/private/factory constructors, generic and nullable representations, implements clauses, member access and erasure).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/extension_type/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
extension type Meters(int value) {
  Meters operator +(Meters other) => Meters(value + other.value);

  int get raw => value;
}

extension type Identifier<T>(T _id) {
  T get id => _id;
}

extension type const Seconds._(int value) {
  Seconds(this.value);

  factory Seconds.zero() => const Seconds._(0);
}

extension type NullableId(int? _value) {
  bool get isSet => _value != null;
}

extension type Wrapped<T extends Object?>._(List<T> _items) {
  Wrapped() : this._(<T>[]);

  void add(T item) => _items.add(item);

  int get length => _items.length;
}

extension type Port(int value) implements int {}

void main() {
  final a = Meters(3);
  final b = Meters(4).raw;
  final sum = a + Meters(b);
  final id = Identifier<String>('x');
  final zero = Seconds.zero();
  final two = Seconds(2);
  final nil = NullableId(null);
  final set = NullableId(5);
  final wrapped = Wrapped<int>()
    ..add(1)
    ..add(2);
  final port = Port(8080);
  final asInt = port + 1;
  if (sum.raw +
          b +
          id.id.length +
          zero.value +
          two.value +
          (nil.isSet ? 0 : 1) +
          (set.isSet ? 1 : 0) +
          wrapped.length +
          asInt !=
      19) {
    throw StateError('unreachable');
  }
}
