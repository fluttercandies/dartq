// Valid Dart 3.15 acceptance corpus — triple shift (shipped, enabledIn
// 2.14): the `>>>` operator declaration, `>>>=` assignment, precedence
// relative to `+`, `&`, `>>`, and `<<`, the `#>>>` symbol literal, and an
// extension-declared `>>>`.
//
// Provenance: shapes modeled on the official language test
// upstream Dart SDK source, tests/language/operator/operator_triple_shift_test.dart
// (SDK main @ e6f4207a). Covered by scripts/acceptance-gate.sh
// (zero-diagnostics bar, `dartq check` machine face).

class C {
  static int ctr = 0;
  final Object? text;
  C([this.text]);

  C operator >>>(C other) => C('(${++ctr}:$text>>>${other.text})');
  C operator >>(C other) => C('(${++ctr}:$text>>${other.text})');
  C operator <<(C other) => C('(${++ctr}:$text<<${other.text})');
  C operator +(C other) => C('(${++ctr}:$text+${other.text})');
  C operator &(C other) => C('(${++ctr}:$text&${other.text})');

  @override
  String toString() => '$text';
}

extension ShiftMany<T> on T {
  List<T> operator >>>(int count) => List<T>.filled(count, this);
}

void main() {
  final c = C(1) >>> C(2);
  var shifted = C(3);
  shifted >>>= C(4);
  final precedence = C(5) + C(6) >>> C(7);
  final samePrecedence = C(8) >>> C(9) >> C(10);
  final withAmpersand = C(11) >>> C(12) & C(13);
  final symbol = #>>>;
  final expanded = Object() >>> 3;

  if (c.text == null ||
      shifted.text == null ||
      precedence.text == null ||
      samePrecedence.text == null ||
      withAmpersand.text == null ||
      !identical(symbol, const Symbol('>>>')) ||
      expanded.length != 3) {
    throw StateError('unreachable');
  }
}
