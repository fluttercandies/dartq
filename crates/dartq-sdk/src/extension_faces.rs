//! Bundled declaration source for the `dart:` extension declarations whose
//! members apply implicitly to receiver types.
//!
//! The generated [`FLAT_LIBRARIES`] table answers where a `dart:` library
//! lives; the generated export-face table the engine builds its synthetic
//! `<dart:…>` nodes from answers which top-level names a `dart:` import
//! exposes. Neither carries element models, so extension *members* — which
//! surface on receiver types, not in import namespaces — resolve only when
//! an element model for the extension declaration reaches the semantic
//! extension pool. This table carries those declarations as checked-in Dart
//! source, in the same `(dart: library name, declaration excerpt)` shape the
//! engine's bundled SDK extension faces iterate (the consumer scans +
//! collects each excerpt once per analyze and indexes it under the
//! library's synthetic face file — see
//! `dartq-engine::sdk_extension_faces`).
//!
//! Source fidelity: verbatim member surfaces from the upstream Dart SDK
//! source, `sdk/lib/core/enum.dart` — `EnumName` (:137) and `EnumByName`
//! (:170) at the oracle of record (3.13.1). Members carry the exact
//! on-types, type-parameter bounds, names, kinds, return types, and formals
//! of the upstream declarations; concrete bodies are spelled `external`
//! (the name-level model is identical and no body is captured either way,
//! the engine excerpts' established ruling). `EnumByName.byName` missing
//! from the extension pool over-fired `undefined_method` on
//! `values.byName(...)` and on `List<T>` receivers whose type parameter is
//! bound by `extends Enum`.
//!
//! Only libraries whose corpus-visible surface includes implicit-application
//! members join — the table is empty of libraries whose face is name-only
//! complete, and it is hand-curated (a small, fidelity-reviewed surface),
//! unlike the generated bulk tables.

/// Per-library bundled extension-face source: `(dart: library name, the
/// declaration excerpt)`. Each entry's library must be addressable — a name
/// of the generated [`FLAT_LIBRARIES`] surface, so the consumer's synthetic
/// `<dart:{library}>` face file pairs with a library the graph build knows.
pub static EXTENSION_FACE_SOURCES: &[(&str, &str)] = &[
    (
        "core",
        r#"extension EnumName on Enum {
  external String get name;
}
extension EnumByName<T extends Enum> on Iterable<T> {
  external T byName(String name);
  external Map<String, T> asNameMap();
}
// The container declarations the member walk needs on a `List`/`Iterable`
// receiver — upstream Dart SDK source, sdk/lib/core/iterable.dart,
// sdk/lib/core/list.dart, sdk/lib/core/set.dart, sdk/lib/core/map.dart.
// Bodies are `external` (the name-level model is identical and no body is
// captured either way). `List`/`Set` keep only the `Iterable` implements
// edge — their own declared members are `List`/`Set`-only spellings the walk
// reads from this stub, never the frozen member table.
abstract class Iterable<E> {
  external Iterator<E> get iterator;
  external Iterable<T> map<T>(T toElement(E element));
  external Iterable<E> where(bool test(E element));
  external Iterable<T> whereType<T>();
  external Iterable<T> expand<T>(Iterable<T> toElements(E element));
  external Iterable<R> cast<R>();
  external Iterable<E> followedBy(Iterable<E> other);
  external Iterable<E> take(int count);
  external Iterable<E> takeWhile(bool test(E value));
  external Iterable<E> skip(int count);
  external Iterable<E> skipWhile(bool test(E value));
  external void forEach(void action(E element));
  external bool any(bool test(E element));
  external bool every(bool test(E element));
  external bool contains(Object? element);
  external T fold<T>(T initialValue, T combine(T previousValue, E element));
  external E reduce(E combine(E value, E element));
  external String join([String separator]);
  external E firstWhere(bool test(E element), {E orElse()?});
  external E lastWhere(bool test(E element), {E orElse()?});
  external E singleWhere(bool test(E element), {E orElse()?});
  external E elementAt(int index);
  external E get first;
  external E get last;
  external E get single;
  external int get length;
  external bool get isEmpty;
  external bool get isNotEmpty;
  external List<E> toList({bool growable});
  external Set<E> toSet();
}
abstract class List<E> implements Iterable<E> {
  external factory List.filled(int length, E fill, {bool growable});
  external factory List.empty({bool growable});
  external factory List.from(Iterable<Object?> elements, {bool growable});
  external factory List.of(Iterable<E> elements, {bool growable});
  external factory List.generate(int length, E generator(int index), {bool growable});
  external factory List.unmodifiable(Iterable elements);
  external List<R> cast<R>();
  external E operator [](int index);
  external void operator []=(int index, E value);
  external List<E> operator +(List<E> other);
  external void set first(E value);
  external E get first;
  external void set last(E value);
  external E get last;
  external int get length;
  external void set length(int newLength);
  external void add(E element);
  external void addAll(Iterable<E> elements);
  external Iterable<E> get reversed;
  external void sort([int compare(E a, E b)?]);
  external void shuffle([Random? random]);
  external int indexOf(E element, [int start]);
  external int indexWhere(bool test(E element), [int start]);
  external int lastIndexWhere(bool test(E element), [int? start]);
  external int lastIndexOf(E element, [int? start]);
  external void clear();
  external void insert(int index, E element);
  external void insertAll(int index, Iterable<E> elements);
  external void setAll(int index, Iterable<E> elements);
  external bool remove(Object? value);
  external void removeWhere(bool test(E element));
  external void retainWhere(bool test(E element));
  external E removeAt(int index);
  external E removeLast();
  external List<E> sublist(int start, [int? end]);
  external Iterable<E> getRange(int start, int end);
  external void setRange(int start, int end, Iterable<E> items, [int skipCount]);
  external void removeRange(int start, int end);
  external void fillRange(int start, int end, [E? fill]);
  external void replaceRange(int start, int end, Iterable<E> replacement);
  external Map<int, E> asMap();
}
// sdk/lib/core/set.dart — `Set` keeps the `Iterable` implements edge for
// the member walk; its own declared members join for the `Set`-only
// spellings the walk reads here.
abstract class Set<E> implements Iterable<E> {
  external factory Set();
  external factory Set.identity();
  external factory Set.from(Iterable elements);
  external factory Set.of(Iterable<E> elements);
  external factory Set.unmodifiable(Iterable<E> elements);
  external Set<R> cast<R>();
  external Iterator<E> get iterator;
  external bool contains(Object? value);
  external bool add(E value);
  external void addAll(Iterable<E> elements);
  external bool remove(Object? value);
  external E? lookup(Object? object);
  external void removeAll(Iterable<Object?> elements);
  external void retainAll(Iterable<Object?> elements);
  external void removeWhere(bool test(E element));
  external void retainWhere(bool test(E element));
  external bool containsAll(Iterable<Object?> other);
  external Set<E> intersection(Set<Object?> other);
  external Set<E> union(Set<E> other);
  external Set<E> difference(Set<Object?> other);
  external void clear();
  external Set<E> toSet();
}
// sdk/lib/core/map.dart — `Map`'s full member surface faces the
// closure-inference walk reads; `K`/`V` are the container's own parameters,
// seeded by the receiver's written instantiation.
abstract class Map<K, V> {
  external factory Map();
  external factory Map.from(Map other);
  external factory Map.of(Map<K, V> other);
  external factory Map.unmodifiable(Map<dynamic, dynamic> other);
  external factory Map.fromIterable(Iterable iterable, {K key(dynamic element)?, V value(dynamic element)?});
  external factory Map.fromIterables(Iterable<K> keys, Iterable<V> values);
  external factory Map.fromEntries(Iterable<MapEntry<K, V>> entries);
  external Map<RK, RV> cast<RK, RV>();
  external bool containsValue(Object? value);
  external bool containsKey(Object? key);
  external V? operator [](Object? key);
  external void operator []=(K key, V value);
  external Iterable<MapEntry<K, V>> get entries;
  external Map<K2, V2> map<K2, V2>(MapEntry<K2, V2> convert(K key, V value));
  external void addEntries(Iterable<MapEntry<K, V>> newEntries);
  external V update(K key, V update(V value), {V ifAbsent()?});
  external void updateAll(V update(K key, V value));
  external void removeWhere(bool test(K key, V value));
  external V putIfAbsent(K key, V ifAbsent());
  external void addAll(Map<K, V> other);
  external V? remove(Object? key);
  external void clear();
  external void forEach(void action(K key, V value));
  external Iterable<K> get keys;
  external Iterable<V> get values;
  external int get length;
  external bool get isEmpty;
  external bool get isNotEmpty;
}
// sdk/lib/core/map.dart — the `MapEntry` container the `Map.entries`/`map`
// spellings reference.
final class MapEntry<K, V> {
  external const MapEntry(this.key, this.value);
  external final K key;
  external final V value;
}
// Upstream Dart SDK source, sdk/lib/collection/iterable.dart:22-104 — the
// `firstOrNull` family, re-exported by dart:core (core.dart:182 `export
// "dart:collection" show NullableIterableExtensions, IterableExtensions`),
// so the implicit dart:core import carries both faces to every file. They
// join under the `core` excerpt alone: a `dart:collection` importer reaches
// them through its own implicit core edge, and a second copy under the
// `collection` face file would double the pool's same-named faces (the
// ambiguity face the library-disjointness contract pins). Bodies spell
// `external` (the name-level model is identical and no body is captured
// either way). The extensions' `on` heads resolve against the excerpt's
// own `Iterable` stub above.
extension NullableIterableExtensions<T extends Object> on Iterable<T?> {
  external Iterable<T> get nonNulls;
}
extension IterableExtensions<T> on Iterable<T> {
  external Iterable<(int, T)> get indexed;
  external T? get firstOrNull;
  external T? get lastOrNull;
  external T? get singleOrNull;
  external T? elementAtOrNull(int index);
}
// sdk/lib/collection/hash_map.dart, linked_hash_map.dart, splay_tree.dart,
// hash_set.dart, queue.dart, collections.dart — the `dart:collection`
// containers the corpus receivers name. Each declares only the `implements`
// edge into the `dart:core` stub above; the member walk climbs it for the
// `forEach`/`map`/`fold`/`keys`/`values`/`[]` faces the receiver's own
// declaration does not carry. These live under the `collection` library —
// the excerpt's own import sees them through the `dart:collection` export
// scope.
"#,
    ),
    (
        "collection",
        r#"abstract final class HashMap<K, V> implements Map<K, V> {
  external factory HashMap();
  external factory HashMap.from(Map other);
  external factory HashMap.of(Map<K, V> other);
}
abstract final class LinkedHashMap<K, V> implements Map<K, V> {
  external factory LinkedHashMap();
  external factory LinkedHashMap.from(Map other);
  external factory LinkedHashMap.of(Map<K, V> other);
}
final class SplayTreeMap<K, V> implements Map<K, V> {
  external factory SplayTreeMap([int compare(K key1, K key2)?, bool isValidKey(Object? potentialKey)?]);
  external factory SplayTreeMap.from(Map other, [int compare(K key1, K key2)?, bool isValidKey(Object? potentialKey)?]);
  external factory SplayTreeMap.of(Map<K, V> other, [int compare(K key1, K key2)?, bool isValidKey(Object? potentialKey)?]);
}
abstract final class HashSet<E> implements Set<E> {
  external factory HashSet();
  external factory HashSet.from(Iterable elements);
  external factory HashSet.of(Iterable<E> elements);
  external factory HashSet.identity();
}
abstract final class LinkedHashSet<E> implements Set<E> {
  external factory LinkedHashSet();
  external factory LinkedHashSet.from(Iterable elements);
  external factory LinkedHashSet.of(Iterable<E> elements);
  external factory LinkedHashSet.identity();
}
final class SplayTreeSet<E> implements Set<E> {
  external factory SplayTreeSet([int compare(E key1, E key2)?, bool isValidKey(Object? potentialKey)?]);
  external factory SplayTreeSet.from(Iterable elements, [int compare(E key1, E key2)?, bool isValidKey(Object? potentialKey)?]);
  external factory SplayTreeSet.of(Iterable<E> elements, [int compare(E key1, E key2)?, bool isValidKey(Object? potentialKey)?]);
}
abstract class Queue<E> implements Iterable<E> {
  external factory Queue();
  external factory Queue.from(Iterable elements);
  external factory Queue.of(Iterable<E> elements);
  external void add(E value);
  external void addFirst(E value);
  external void addLast(E value);
  external void addAll(Iterable<E> iterable);
  external bool remove(Object? object);
  external void removeWhere(bool test(E element));
  external void retainWhere(bool test(E element));
  external void clear();
  external E removeFirst();
  external E removeLast();
  external E get first;
  external E get last;
  external bool get isEmpty;
  external int get length;
  external Queue<R> cast<R>();
}
final class ListQueue<E> implements Queue<E> {
  external factory ListQueue([int? initialCapacity]);
  external factory ListQueue.from(Iterable elements);
  external factory ListQueue.of(Iterable<E> elements);
}
final class DoubleLinkedQueue<E> implements Queue<E> {
  external factory DoubleLinkedQueue();
  external factory DoubleLinkedQueue.from(Iterable elements);
  external factory DoubleLinkedQueue.of(Iterable<E> elements);
}
base class UnmodifiableMapView<K, V> implements Map<K, V> {
  external UnmodifiableMapView(Map<K, V> map);
}
base class UnmodifiableSetView<E> implements Set<E> {
  external UnmodifiableSetView(Set<E> source);
}
base class UnmodifiableListView<E> implements List<E> {
  external UnmodifiableListView(Iterable<E> source);
}
"#,
    ),
];

#[cfg(test)]
mod tests {
    use super::EXTENSION_FACE_SOURCES;

    /// Every entry's library is addressable: the consumer interns the
    /// excerpt under the synthetic `<dart:{library}>` face file, and that
    /// id only pairs with the graph's SDK nodes when the library name is
    /// one the generated surfaces know (`FLAT_LIBRARIES` is the in-crate
    /// face of the same generated SDK table the engine's export-face list
    /// is built from).
    #[test]
    fn entries_reference_addressable_libraries() {
        for (library, source) in EXTENSION_FACE_SOURCES {
            assert!(
                crate::FLAT_LIBRARIES
                    .iter()
                    .any(|(name, _)| name == library),
                "extension face library `{library}` is not an addressable dart: library"
            );
            assert!(!source.is_empty(), "extension face `{library}` is empty");
        }
    }

    /// A library joins the table at most once: the consumer indexes each
    /// excerpt under the library's face file, and a duplicate entry would
    /// silently overwrite the earlier face's element model.
    #[test]
    fn library_names_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for (library, _) in EXTENSION_FACE_SOURCES {
            assert!(
                seen.insert(*library),
                "duplicate extension face `{library}`"
            );
        }
    }

    /// The `core` excerpt carries the dart:core enum extension pair with
    /// the upstream heads (exact on-types and bounds) and the exact
    /// member surfaces — `EnumName.name`,
    /// `EnumByName.byName`/`EnumByName.asNameMap` (upstream Dart SDK
    /// source, sdk/lib/core/enum.dart:137/:170). These spellings are the
    /// contract the semantic extension pool resolves against; any drift
    /// here is a parity regression on the `values.byName(...)` face.
    #[test]
    fn core_excerpt_carries_the_enum_extension_faces() {
        let core = EXTENSION_FACE_SOURCES
            .iter()
            .find(|(library, _)| *library == "core")
            .expect("core extension face present")
            .1;
        // Exact declaration heads: on-types and the `T extends Enum` bound.
        assert!(
            core.contains("extension EnumName on Enum {"),
            "EnumName head drifted"
        );
        assert!(
            core.contains("extension EnumByName<T extends Enum> on Iterable<T> {"),
            "EnumByName head or bound drifted"
        );
        // Member surfaces: kinds, names, return types, formals.
        assert!(
            core.contains("external String get name;"),
            "EnumName.name surface drifted"
        );
        assert!(
            core.contains("external T byName(String name);"),
            "EnumByName.byName surface drifted"
        );
        assert!(
            core.contains("external Map<String, T> asNameMap();"),
            "EnumByName.asNameMap surface drifted"
        );
    }
}
