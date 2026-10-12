// Valid Dart 3.15 acceptance corpus — wildcard variables (non-binding `_` parameters (repeated), wildcard for-in elements, wildcard list/map patterns, wildcard catch clauses, private `_`-prefixed locals kept binding).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/wildcard_variables/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
int positionalSum(int a, int _, int c) => a + c;

void twoWildcards(Object _, Object _2, Object _) {}

class Bag {
  final int _size;

  Bag(int _initial, this._size);

  int get size => _size;
}

void sweep(List<int> values) {
  var kept = 0;
  for (final _ in values) {
    kept++;
  }
  final [_, second, ...] = values;
  final {'k': _, 'j': joint} = {'k': 1, 'j': 2};
  if (second + joint == kept) {}
}

String catchGuard(Object Function() run) {
  try {
    return 'ok:${run()}';
  } on StateError catch (_) {
    return 'state-error';
  } catch (_e) {
    return 'other:${_e.runtimeType}';
  }
}

void main() {
  final sum = positionalSum(1, 99, 3);
  twoWildcards(1, 2, 3);
  final bag = Bag(0, 5);
  sweep([1, 2, 3, 4]);
  final guarded = catchGuard(() => throw StateError('bad'));
  final plain = catchGuard(() => 'fine');
  final items = <Object>[sum, bag.size, guarded, plain];
  if (items.length != 4) {
    throw StateError('unreachable');
  }
}
