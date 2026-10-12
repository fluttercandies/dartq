// Valid Dart 3.15 acceptance corpus — number literals (decimal/hex ints with leading zeros, doubles, signed/unsigned exponents, mantissa-exponent forms, int-exponent doubles, const numeric class fields).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/number/, tests/language/double_literals/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
const decimal = 42;

const zero = 0;

const leadingZeros = 007;

const hexLower = 0xff;

const hexUpper = 0X1A;

const hexLong = 0xDEADBEEF;

const smallDouble = 1.5;

const wholeDouble = 2.0;

const exponentLower = 1e3;

const exponentUpper = 1E3;

const exponentSigned = 1e+3;

const exponentNegative = 5e-2;

const mantissaExponent = 1.25e4;

const intExponent = 1e0;

class Physics {
  static const double gravity = 9.81;

  static const lightSpeed = 299792458;

  static const planck = 6.62607015e-34;
}

void main() {
  const computed = decimal + hexLower ~/ leadingZeros;
  final doubles = <double>[
    smallDouble,
    wholeDouble,
    exponentLower,
    exponentUpper,
    exponentSigned,
    exponentNegative,
    mantissaExponent,
    intExponent.toDouble(),
    Physics.gravity,
    Physics.planck,
  ];
  final ints = <int>[
    decimal,
    zero,
    leadingZeros,
    hexLower,
    hexUpper,
    hexLong,
    Physics.lightSpeed,
    computed,
  ];
  final negative = -decimal;
  final doubleNegative = -smallDouble;
  if (doubles.length + ints.length + negative + doubleNegative.toInt() != 18) {
    throw StateError('unreachable');
  }
}
