/// Deeply nested generic option matrices: variant trees stored as maps of
/// lists of maps, plus the generic container that walks them.
library;

/// A path through an option tree: layer name -> option -> concrete values.
typedef OptionPath = List<Map<String, List<String>>>;

/// Groups selectable options for one SKU family. The field type nests four
/// type arguments deep; every signature stays fully generic.
class OptionMatrix<T extends Object> {
  final Map<String, List<Map<String, List<T>>>> layers;

  const OptionMatrix(this.layers);

  bool get isEmpty => layers.isEmpty;

  /// All leaf values in layer iteration order.
  Iterable<T> leaves() sync* {
    for (final entry in layers.entries) {
      for (final inner in entry.value) {
        for (final values in inner.values) {
          yield* values;
        }
      }
    }
  }

  /// Resolves the first layer whose option map contains [wanted] under any
  /// key; generic in the bound type used for the comparison.
  M? resolveFirst<M extends T>(M wanted) {
    for (final inner in layers.values) {
      for (final optionMap in inner) {
        for (final values in optionMap.values) {
          if (values.contains(wanted)) {
            return wanted;
          }
        }
      }
    }
    return null;
  }

  /// Rebuilds the matrix with each leaf mapped through [transform].
  OptionMatrix<R> mapLeaves<R extends Object>(R Function(T leaf) transform) {
    final mapped = <String, List<Map<String, List<R>>>>{
      for (final entry in layers.entries)
        entry.key: [
          for (final optionMap in entry.value)
            {
              for (final optionEntry in optionMap.entries)
                optionEntry.key: [
                  for (final leaf in optionEntry.value) transform(leaf),
                ],
            },
        ],
    };
    return OptionMatrix<R>(mapped);
  }
}

/// Flattening helpers kept as a named extension so call sites read clearly.
extension OptionMatrixFlatten<T extends Object> on OptionMatrix<T> {
  Map<String, List<T>> flatByLayer() {
    final result = <String, List<T>>{};
    for (final entry in layers.entries) {
      final flat = <T>[];
      for (final optionMap in entry.value) {
        for (final values in optionMap.values) {
          flat.addAll(values);
        }
      }
      result[entry.key] = flat;
    }
    return result;
  }
}
