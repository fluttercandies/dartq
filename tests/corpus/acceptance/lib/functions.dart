// Valid Dart 3.15 acceptance corpus — functions (closures over all parameter shapes, generic function type aliases as parameters, constructor tear-offs with explicit instantiation, generic function instantiation, callable classes, named arguments in any position).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/function/, tests/language/named_arguments_anywhere/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
typedef Reducer<T> = T Function(T acc, T next);

T foldList<T>(List<T> items, T seed, Reducer<T> reduce) {
  var acc = seed;
  for (final item in items) {
    acc = reduce(acc, item);
  }
  return acc;
}

int positional({required int first, int second = 0, int? third}) =>
    first + second + (third ?? 0);

void namedAnywhere() {
  final r1 = positional(first: 1, second: 2);
  final r2 = positional(second: 5, first: 1);
  if (r1 + r2 == 0) {}
}

class Greeter {
  final String who;

  Greeter(this.who);

  String call([String greeting = 'hi']) => '$greeting, $who!';
}

int apply(int Function(int) f, int x) => f(x);

void main() {
  final sum = foldList<int>([1, 2, 3], 0, (acc, next) => acc + next);
  final joined = foldList<String>(
    ['a', 'b'],
    '',
    (acc, next) => next + acc,
  );
  final instantiations = <Object>[
    foldList<int>,
    positional,
    apply,
  ];
  final listCtor = List<int>.filled;
  final filled = listCtor(2, 7);
  final maxTear = foldList<int>;
  final genericTorn = <int>[3, 1].fold<int>;
  final greeter = Greeter('ada');
  final greeted = greeter();
  final greetedLoud = greeter('HELLO');
  final doubled = apply((n) => n * 2, 5);
  final piped = apply((int n) {
    return n + 1;
  }, 9);
  final closures = <Object>[
    () => 1,
    (int a, {int b = 2}) => a + b,
    ([int? c]) => c ?? 3,
    (int a, [int b = 1]) => a - b,
  ];
  final everything = <Object>[
    sum,
    joined,
    instantiations,
    filled,
    maxTear,
    genericTorn,
    greeted,
    greetedLoud,
    doubled,
    piped,
    closures,
  ];
  if (everything.length != 11) {
    throw StateError('unreachable');
  }
}
