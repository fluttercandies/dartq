// Valid Dart 3.15 acceptance corpus — typedefs (old-style function aliases, new general typedefs, generic typedefs, nullable function types, typedefs of records, typedef-typed members).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/typedef/, tests/language/nonfunction_type_aliases/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
typedef IntPredicate = bool Function(int);

typedef Comparer<T> = int Function(T a, T b);

typedef void LegacyCallback(int value);

typedef NamedParams = void Function({required String name, int? age});

typedef NullableMapper = String? Function(int?)?;

typedef Raw = Function;

typedef AnyGeneral = dynamic;

class Registry {
  final Map<String, IntPredicate> rules = <String, IntPredicate>{};

  void register(String key, IntPredicate rule) => rules[key] = rule;

  bool check(String key, int value) => rules[key]!(value);
}

typedef PairOf<T> = (T, T);

typedef NamedRecord = ({int id, String label});

PairOf<int> makePair(int x) => (x, x);

void main() {
  final isEven = (int v) => v.isEven;
  final registry = Registry()..register('even', isEven);
  Comparer<int> ascending = (a, b) => a.compareTo(b);
  LegacyCallback cb = (v) {
    if (v < 0) {}
  };
  NamedParams named = ({required String name, int? age}) {
    if (age != null) {}
  };
  NullableMapper mapper = (int? v) => v == null ? null : '$v';
  final Raw raw = ascending;
  final pair = makePair(3);
  const label = 'entry';
  final result = <Object>[
    registry,
    registry.check('even', 2),
    ascending(1, 2),
    cb,
    named,
    mapper,
    raw,
  ];
  final NamedRecord record = (id: pair.$1, label: label);
  final AnyGeneral anything = result;
  if (anything is! List<Object> || record.id != 3 || result.length != 7) {
    throw StateError('unreachable');
  }
}
