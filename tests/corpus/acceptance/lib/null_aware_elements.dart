// Valid Dart 3.15 acceptance corpus — null-aware elements (shipped,
// enabledIn 3.8.0): null-aware list elements (`?e`), set elements, map
// keys (`?k: v`), and map values (`k: ?v`), in mutable and const literals.
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/null_aware_elements/const_literals_test.dart
// (SDK main @ e6f4207a). Covered by scripts/acceptance-gate.sh
// (zero-diagnostics bar, `dartq check` machine face).

const int? nullConst = null;
const int intConst = 0;

const constList = <int?>[?nullConst, intConst];
const constSet = <int?>{?nullConst, intConst};
const constKeyMap = <int?, int>{?nullConst: 1, intConst: 2};
const constValueMap = <int, int?>{intConst: ?nullConst};

int? evenOrNull(int n) => n.isEven ? n : null;

void main() {
  final a = evenOrNull(2);
  final b = evenOrNull(1);
  final list = <int>[?a, 7, ?b];
  final set = <int>{?a, ?b, 7};
  final keyMap = <int, int>{?a: 1, ?b: 2, 7: 3};
  final valueMap = <int, int?>{1: ?a, 2: ?b, 3: 7};
  final consts = <Object>[constList, constSet, constKeyMap, constValueMap];

  if (list.length +
          set.length +
          keyMap.length +
          valueMap.length +
          consts.length !=
      12) {
    throw StateError('unreachable');
  }
}
