// Valid Dart 3.15 acceptance corpus — constants (const constructors with field initializers, static const members, canonicalized instances and collections, const ternary and interpolation, const sets of records, potentially-const contexts).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/const/, tests/language/constants_2018/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
class Complex {
  final double real;
  final double imaginary;

  const Complex(this.real, this.imaginary);

  static const zero = Complex(0, 0);

  const Complex.one() : real = 1, imaginary = 0;

  Complex.scaled(Complex base, double factor)
      : real = base.real * factor,
        imaginary = base.imaginary * factor;

  Complex operator +(Complex other) =>
      Complex(real + other.real, imaginary + other.imaginary);
}

const unit = Complex.one();

final scaled = Complex.scaled(Complex(2, 3), 1.5);

const canonicalA = Complex(1, 2);

const canonicalB = Complex(1, 2);

const ternary = true;

const chosen = ternary ? canonicalA : canonicalB;

const interpolatedConst = 'x=$ternary';

final interpolated = 'r=${canonicalA.real}, i=${canonicalA.imaginary}';

const nestedList = <List<int>>[
  <int>[1, 2],
  <int>[3],
];

const deepMap = <String, Map<String, int>>{
  'a': {'x': 1},
  'b': {'y': 2},
};

const canonicalSets = <int>{1, 2, 3};

const canonicalPairs = <(int, int)>{(1, 2), (3, 4)};

void potentiallyConst(double f) {
  const zero = Complex.zero;
  final local = ternary ? Complex.one() : Complex(2, 3);
  if (zero.real + local.imaginary + f == double.infinity) {}
}

void main() {
  final isIdentical = identical(canonicalA, canonicalB);
  final sameSet = identical(canonicalSets, const <int>{1, 2, 3});
  final added = canonicalA + canonicalB;
  final all = <Object>[
    unit,
    scaled,
    chosen,
    interpolated,
    interpolatedConst,
    nestedList,
    deepMap,
    canonicalSets,
    canonicalPairs,
    isIdentical,
    sameSet,
    added,
  ];
  potentiallyConst(2.5);
  if (all.length != 12) {
    throw StateError('unreachable');
  }
}
