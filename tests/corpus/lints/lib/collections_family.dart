// collections and const-new family — fixture wave 2 kitchen file.
// add/addAll partition between prefer_inlined_adds (list-literal
// argument) and prefer_spread_collections (non-literal argument); the
// cascade faces here are initializers, not expression statements, so
// avoid_single_cascade gets its own expression-statement face.
// Expected rows: 9, all keyed collections_family.dart —
// PREFER_COLLECTION_LITERALS (two faces: `Map()` and `Set()`, the no-arg
// creations; written HashMap/HashSet never report on the oracle),
// PREFER_INLINED_ADDS (<int>[]..add(1)), PREFER_SPREAD_COLLECTIONS
// (<int>[]..addAll(more), non-literal argument), 
// UNNECESSARY_TO_LIST_IN_SPREADS ([...xs.toList()]),
// AVOID_SINGLE_CASCADE_IN_EXPRESSION_STATEMENTS (buffer..write('x');),
// UNNECESSARY_NEW (new List<int>.filled — the `new` keyword; List.filled
// is outside prefer_collection_literals' face), UNNECESSARY_CONST (the
// inner `const` inside the const variable's initializer context),
// PREFER_IS_EMPTY (xs.length == 0), PREFER_IS_NOT_EMPTY (!xs.isEmpty).
final table = Map();

final group = Set();

final queued = <int>[]..add(1);

final filled = new List<int>.filled(2, 0);

const digits = const <int>[1, 2];

void spreadUser(List<int> more) {
  final merged = <int>[]..addAll(more);
  print(merged);
}

void copyUser(List<int> xs) {
  final copied = [...xs.toList()];
  print(copied);
}

void bufferUser() {
  final buffer = StringBuffer();
  buffer..write('x');
}

void emptyUser(List<int> xs) {
  if (xs.length == 0) {
    print(0);
  }
}

void notEmptyUser(List<int> xs) {
  if (!xs.isEmpty) {
    print(1);
  }
}
