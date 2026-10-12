// Valid Dart 3.15 acceptance corpus — collection literals (typed/const list/set/map literals, spread and null-aware spread, collection-if with else, collection-for (C-style and for-in), elements with patterns, nesting).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/control_flow_collections/, tests/language/spread_collections/, tests/language/set_literals/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
const baseFibs = <int>[1, 1, 2, 3];

const moreFibs = <int>[...baseFibs, 5, 8];

const maybeFibs = <int>[...?null, ...baseFibs];

const conditional = <String>[
  'always',
  if (2 > 1) 'two gt one' else 'impossible',
];

final forGenerated = <int>[for (var i = 0; i < 3; i++) i * i];

const constMap = <String, int>{'a': 1, 'b': 2};

const spreadMap = <String, int>{...constMap, 'c': 3};

const constSet = <String>{'x', 'y', 'z'};

final setOf = <String>{'x', 'y', 'x'.toUpperCase()};

List<int> evens(List<int> source) => [
      for (final v in source)
        if (v.isEven) v,
    ];

Map<String, List<int>> grouped(List<int> source) => {
      for (final v in source) '${v % 2}': <int>[v],
    };

Set<String> tagged(List<int> source) => {
      for (final v in source) 'tag-$v',
      if (source.isEmpty) 'empty',
    };

void main() {
  final grown = <int>[...moreFibs, ...?maybeNull()];
  final localMap = <String, List<int>>{
    'evens': evens(forGenerated),
    'rest': [for (var i = 10; i < 13; i++) i],
  };
  final tags = tagged(baseFibs);
  final groups = grouped([1, 2, 3, 4]);
  final flat = <Object>[
    ...grown,
    ...conditional,
    ...forGenerated,
  ];
  final maps = <Map<String, int>>[constMap, spreadMap, const {}];
  final sets = <Set<String>>{setOf, constSet, {'a'}};
  if (localMap['evens']!.length +
          groups.length +
          tags.length +
          flat.length +
          maps.length +
          sets.length +
          grown.length !=
      32) {
    throw StateError('unreachable');
  }
}

List<int>? maybeNull() => null;
