// Valid Dart 3.15 acceptance corpus — operators (full overloadable-operator set (+ - * / ~/ % unary- ~ | & ^ << >> >>> == compareTo), [] and []= pairs, callable classes via call, ++/--/--= usage on built-ins).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/operator/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
class Money implements Comparable<Money> {
  final int cents;

  const Money(this.cents);

  Money operator +(Money other) => Money(cents + other.cents);

  Money operator -(Money other) => Money(cents - other.cents);

  Money operator *(int factor) => Money(cents * factor);

  Money operator ~/(int divisor) => Money(cents ~/ divisor);

  double operator /(int divisor) => cents / divisor;

  Money operator %(int divisor) => Money(cents % divisor);

  Money operator -() => Money(-cents);

  Money operator ~() => Money(~cents);

  Money operator |(Money other) => Money(cents | other.cents);

  Money operator &(Money other) => Money(cents & other.cents);

  Money operator ^(Money other) => Money(cents ^ other.cents);

  Money operator <<(int shift) => Money(cents << shift);

  Money operator >>(int shift) => Money(cents >> shift);

  Money operator >>>(int shift) => Money(cents >>> shift);

  @override
  bool operator ==(Object other) => other is Money && other.cents == cents;

  @override
  int compareTo(Money other) => cents.compareTo(other.cents);
}

class Matrix {
  final List<List<int>> cells;

  Matrix(this.cells);

  int operator [](int row) => cells[row][0];

  void operator []=(int row, int value) => cells[row][0] = value;
}

class Counter {
  int value = 0;

  void call([int step = 1]) => value += step;
}

void main() {
  const a = Money(100);
  const b = Money(30);
  final added = a + b;
  final subtracted = a - b;
  final scaled = a * 2;
  final floored = a ~/ 3;
  final divided = a / 4;
  final remainder = a % 7;
  final negated = -a;
  final complemented = ~a;
  final or = a | b;
  final and = a & b;
  final xor = a ^ b;
  final left = a << 1;
  final right = a >> 1;
  final unsigned = a >>> 1;
  final equal = a == Money(100).negate();
  final compare = a.compareTo(b);
  final matrix = Matrix([
    [1],
    [2],
  ]);
  matrix[1] = 9;
  final first = matrix[0];
  final counter = Counter();
  counter();
  counter(5);
  var counted = 0;
  counted++;
  counted--;
  counted += 2;
  final results = <Object>[
    added,
    subtracted,
    scaled,
    floored,
    divided,
    remainder,
    negated,
    complemented,
    or,
    and,
    xor,
    left,
    right,
    unsigned,
    equal,
    compare,
    first,
    matrix,
    counter.value,
    counted,
  ];
  if (results.length != 20) {
    throw StateError('unreachable');
  }
}

extension Negate on Money {
  Money negate() => Money(-cents);
}
