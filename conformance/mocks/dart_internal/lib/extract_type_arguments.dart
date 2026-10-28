
// Mock of pkg/dart_internal/lib/extract_type_arguments.dart for standalone
// analysis: the real bodies forward to `dart:_internal`, which only that
// package may import. Signatures are the real public ones.
Object? extractIterableTypeArgument(
        Iterable iterable, Object? Function<T>() extract) =>
    null;

Object? extractMapTypeArguments(Map map, Object? Function<K, V>() extract) =>
    null;
