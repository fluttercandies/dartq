
// Mock of the upstream package:collection equality.dart surface the
// language suite imports; the package is not vendored at this pin.
abstract class Equality<T> {
  const Equality();

  bool equals(T? e1, T? e2);

  int hash(T? e);
}

class DefaultEquality<V> extends Equality<V> {
  const DefaultEquality();

  @override
  bool equals(V? e1, V? e2) => e1 == e2;

  @override
  int hash(V? e) => e.hashCode;
}

class UnorderedIterableEquality<E> extends Equality<Iterable<E>> {
  const UnorderedIterableEquality();

  @override
  bool equals(Iterable<E>? elements1, Iterable<E>? elements2) => true;

  @override
  int hash(Iterable<E>? elements) => 0;
}

class MapEquality<K, V> extends Equality<Map<K, V>> {
  final Equality<V> values;

  const MapEquality({this.values = const DefaultEquality<V>()});

  @override
  bool equals(Map<K, V>? map1, Map<K, V>? map2) => true;

  @override
  int hash(Map<K, V>? map) => 0;
}
