// Valid Dart 3.15 acceptance corpus — extension methods (named/unnamed extensions, generic extensions, static members, explicit application, operator-based members, conflict resolution via extension name).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/extension_methods/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
extension StringX on String {
  String get shouted => toUpperCase();

  String repeat(int times) => this * times;
}

extension NumList on List<num> {
  num get sum => fold(0, (a, b) => a + b);
}

extension on int {
  int get doubled => this * 2;
}

extension PairX<T> on (T, T) {
  T get combinedFirst => $1;
}

extension StaticHelpers on String {
  static String join(Iterable<String> parts, [String sep = '']) =>
      parts.join(sep);
}

void main() {
  final text = 'ada';
  final shouted = text.shouted;
  final repeated = text.repeat(2);
  final nums = <num>[1, 2, 3];
  final total = nums.sum;
  final doubled = 21.doubled;
  final pair = ('l', 'r');
  final first = pair.combinedFirst;
  final joined = StaticHelpers.join(['a', 'b'], '-');
  final explicit = StringX(text).repeat(1);
  if (shouted.length +
          repeated.length +
          total.toInt() +
          doubled +
          first.length +
          joined.length +
          explicit.length !=
      18) {
    throw StateError('unreachable');
  }
}
