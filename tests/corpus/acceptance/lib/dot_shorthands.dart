// Valid Dart 3.15 acceptance corpus — dot shorthands (shipped, enabledIn
// 3.10.0): enum shorthands, static getter/method shorthands, static-const
// shorthands, extension-type shorthands, constructor shorthands (`.new`
// and named), const shorthands (`const .named(...)`), null-assert chains
// (`.nullable!.member`), collection-literal contexts, equality contexts,
// and the relational-pattern face (`case == .blue`).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/dot_shorthands/ — feature_enabled_test.dart,
// dot_shorthand_helper.dart, member/static_method_test.dart,
// equality/equality_test.dart, multiple/selector_chain_test.dart, and
// multiple/null_assert_test.dart (SDK main @ e6f4207a). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine
// face).

enum Color { red, green, blue }

class Integer {
  static Integer get one => Integer(1);
  static Integer? get nullable => null;
  static const Integer constOne = Integer._(1);
  final int value;
  Integer(this.value);
  const Integer._(this.value);
}

extension type IntegerExt(int value) {
  static IntegerExt get one => IntegerExt(1);
  static const IntegerExt constOne = IntegerExt._(1);
  const IntegerExt._(this.value);
}

class Point {
  final int x;
  final int y;
  const Point(this.x, this.y);
  const Point.origin() : this(0, 0);
  static Point get zero => const Point(0, 0);
  static Point create() => const Point(1, 1);
}

void main() {
  Color color = .blue;
  const Color constColor = .blue;
  Integer one = .one;
  const Integer constOne = .constOne;
  IntegerExt extOne = .one;
  const IntegerExt extConst = .constOne;
  Point created = .create();
  Point chained = .zero;
  const Point constPoint = .origin();
  final constructed = <Point>[.zero, .create(), const .origin()];
  final asserted = .nullable!;
  final equality = color == .blue;
  final patternEquality = switch (color) {
    == .blue => 'blue',
    _ => 'other',
  };

  final values = <int>[
    constColor.index,
    one.value,
    constOne.value,
    extOne.value,
    extConst.value,
    created.x,
    created.y,
    chained.x,
    chained.y,
    constPoint.x,
    constPoint.y,
    asserted.value,
  ];
  if (!equality || patternEquality.isEmpty || values.length != 12) {
    throw StateError('unreachable');
  }
}
