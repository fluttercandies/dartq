// Valid Dart 3.15 acceptance corpus — patterns (all pattern forms).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/patterns/ and the CFE parser suites
// .third/dart-sdk/pkg/front_end/parser_testcases/patterns/ (SDK main @
// e6f4207a). Covered by scripts/acceptance-gate.sh (zero-diagnostics bar,
// `dartq check` machine face).
//
// Forms covered: constant (literal + named), variable (bare/var/final/
// typed), wildcard, parenthesized, logical-or, logical-and, relational
// (== != < > <= >=), cast, null-check, null-assert, list (with rest),
// map, record (positional + named), object (with typed subpatterns and
// relational subpatterns), guards (`when`), switch statement, switch
// expression, if-case, pattern-variable declaration, pattern assignment,
// and for-in patterns.

class Rect {
  final int width;
  final int height;
  const Rect(this.width, this.height);

  int area() => width * height;
}

const int one = 1;

// Constant, variable, typed/final variable, guard, and relational forms.
String classify(Object value) {
  switch (value) {
    case 0:
      return 'zero';
    case one:
      return 'one';
    case int n when n < 0:
      return 'negative $n';
    case int n:
      return 'int $n';
    case final String s:
      return 'string $s';
    case < 10:
      return 'small non-int';
    case var other:
      return 'other $other';
  }
}

// Logical-or, logical-and, and exact-constant forms over a numeric
// scrutinee.
String bands(int n) {
  switch (n) {
    case < 0:
      return 'negative';
    case 0:
      return 'zero';
    case 1 || 2 || 3:
      return 'low';
    case > 3 && < 10:
      return 'mid';
    case 10:
      return 'ten';
    case var rest:
      return 'large $rest';
  }
}

// List (with guard, wildcard, rest), map, record (positional + named
// getters), and object patterns over an untyped scrutinee.
String composites(Object value) {
  switch (value) {
    case [int a, int b] when a == b:
      return 'square $a';
    case [_, var second]:
      return 'pair ending $second';
    case [var first, ...var rest] when rest.isNotEmpty:
      return 'list from $first (+${rest.length})';
    case {'k': int v}:
      return 'map $v';
    case (var a, var b):
      return 'record $a $b';
    case (:name, :age):
      return 'named $name $age';
    case Rect(width: var w, height: > 10):
      return 'tall rect $w';
    case var other:
      return 'unmatched $other';
  }
}

// Null-check pattern over a nullable scrutinee.
String nullFaces(int? maybe) {
  switch (maybe) {
    case int v? when v > 100:
      return 'big $v';
    case null:
      return 'null';
    case var rest:
      return 'small $rest';
  }
}

// Null-assert pattern inside a refutable if-case.
String assertFace((int?, int?) pair) {
  if (pair case (var a!, var b!) when a > b) {
    return 'desc $a $b';
  }
  return 'asc-or-equal';
}

// Cast pattern with a guard.
String castFace(Object value) {
  if (value case var text as String when text.isNotEmpty) {
    return 'text $text';
  }
  return 'not-a-string';
}

// Record pattern-variable declarations (untyped and typed).
int sumPair((int, int) pair) {
  final (a, b) = pair;
  return a + b;
}

int typedSum((int, int) pair) {
  final (int a, int b) = pair;
  return a + b;
}

// Record pattern assignment swapping two variables.
int swapAndSum() {
  var x = 1;
  var y = 2;
  (x, y) = (y, x);
  return x + y;
}

// For-in record pattern.
int sumPairs(List<(int, int)> pairs) {
  var total = 0;
  for (final (a, b) in pairs) {
    total += a + b;
  }
  return total;
}

// Switch expression with relational, logical-and, and wildcard arms.
String sizeClass(int n) => switch (n) {
      < 0 => 'negative',
      0 => 'zero',
      > 0 && < 10 => 'small',
      _ => 'large',
    };

// Switch expression over a record type with a named-field constant
// subpattern.
String namedClassify(({String name, int age}) person) => switch (person) {
      (:name, age: 0) => 'newborn $name',
      (:name, age: var age) => '$name is $age',
    };

void main() {
  final results = <String>[
    classify(0),
    classify(one),
    classify(-5),
    classify(42),
    classify('hi'),
    classify(3.5),
    classify(Rect(3, 4)),
    bands(-1),
    bands(0),
    bands(2),
    bands(5),
    bands(10),
    bands(99),
    composites(const [1, 1]),
    composites(const [1, 2]),
    composites(const [1, 2, 3]),
    composites(const {'k': 7}),
    composites(const (8, 9)),
    composites(const (name: 'ada', age: 36)),
    composites(const Rect(20, 4)),
    composites(true),
    nullFaces(null),
    nullFaces(500),
    nullFaces(3),
    assertFace(const (3, 1)),
    castFace('text'),
    sizeClass(5),
    namedClassify((name: 'bob', age: 0)),
    namedClassify((name: 'cat', age: 2)),
  ];
  final total = sumPair((1, 2)) +
      typedSum((3, 4)) +
      swapAndSum() +
      sumPairs([
        (1, 2),
        (3, 4),
      ]);
  if (results.length != 30 || total != 23) {
    throw StateError('unreachable');
  }
}
