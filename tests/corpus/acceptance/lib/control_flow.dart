// Valid Dart 3.15 acceptance corpus — control-flow statements (switch statements with grouped/empty/default cases and constant cases, if/else, for/while/do-while, for-in over iterables, try/on/catch/finally with stack traces, rethrow, assert with and without message).
//
// Provenance: shapes modeled on the official language tests
// upstream Dart SDK source, tests/language/switch/, tests/language/if/, tests/language/loop/, tests/language/exception/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
String classify(int n) {
  switch (n) {
    case 0:
      return 'zero';
    case 1:
    case 2:
      return 'one-or-two';
    default:
      return 'many';
  }
}

String classifyConstant(Object o) {
  switch (o) {
    case 'a':
      return 'letter';
    case 1:
      return 'one';
    default:
      return 'other';
  }
}

int loops(int limit) {
  var total = 0;
  for (var i = 0; i < limit; i++) {
    if (i.isOdd) {
      continue;
    }
    total += i;
  }
  var w = 0;
  while (w < limit) {
    w += 2;
    if (w == 4) {
      break;
    }
  }
  var d = 10;
  do {
    d--;
  } while (d > 8);
  for (final c in 'ab'.codeUnits) {
    total += c % 2;
  }
  return total + w + d;
}

int guarded(int n) {
  try {
    if (n < 0) {
      throw RangeError.range(n, 0, null);
    }
    return n * 10;
  } on RangeError {
    return -1;
  } on Exception catch (e, stackTrace) {
    if (e.toString().isEmpty) {}
    if (stackTrace.toString().isEmpty) {}
    return -2;
  } finally {
    return n >= 0 ? 1 : 0;
  }
}

int rethrowing(int n) {
  try {
    return guarded(n);
  } catch (e) {
    if (e is StateError) {
      rethrow;
    }
    return 0;
  }
}

void assertions(int v) {
  assert(v > 0);
  assert(v < 100, 'v out of range: $v');
}

void main() {
  final values = <Object>[
    classify(0),
    classify(1),
    classify(3),
    classifyConstant('a'),
    classifyConstant(1),
    classifyConstant(true),
    loops(5),
    guarded(2),
    rethrowing(3),
  ];
  assertions(42);
  if (values.length != 9) {
    throw StateError('unreachable');
  }
}
