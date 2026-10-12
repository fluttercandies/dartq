// Valid Dart 3.15 acceptance corpus — digit separators (shipped, enabledIn
// 3.6.0): `__`/`_` runs in decimal integers, leading zeros, hex literals,
// negative literals, doubles, and exponent notation, plus int-to-double
// coercion contexts and const contexts.
//
// Provenance: shapes modeled on the official language test
// .third/dart-sdk/tests/language/number/separators_test.dart (SDK main @
// e6f4207a). Covered by scripts/acceptance-gate.sh (zero-diagnostics bar,
// `dartq check` machine face).

const int thousand = 1_000;
const double pi3 = 3.141_592_653;

double asDouble(double x) => x;

void main() {
  final big = 1__000_000_000;
  final mixed = 555_867_5309;
  final negative = -1_000_000;
  final leading = 00_1_00;
  final negativeLeading = -00_99;
  final zeros = 0__0__0__0__0;
  final hex = 0x0___F__F;
  final double1 = 1__9.7_3_9;
  final double2 = -1.6_1_8;
  final exponent1 = 1e3_0;
  final exponent2 = 1_2e3;
  final exponent3 = 1_2.3_4e1;
  final zeroDouble = asDouble(0__0);
  double coerced = 1_000;
  const constSep = 2_000;
  const double constDouble = 4_000;

  final okExponents = exponent1 == 1e30 &&
      exponent2 == 12000 &&
      exponent3 == 123.4 &&
      zeroDouble == 0.0 &&
      pi3 > 3 &&
      thousand == 1000;
  final total = big +
      mixed +
      negative +
      leading +
      negativeLeading +
      zeros +
      hex +
      double1.toInt() +
      double2.toInt() +
      coerced.toInt() +
      constSep +
      constDouble.toInt();
  if (total != 6557682583 || !okExponents) {
    throw StateError('unreachable');
  }
}
