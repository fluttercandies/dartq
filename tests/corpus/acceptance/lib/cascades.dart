// Valid Dart 3.15 acceptance corpus — cascades (cascade sections (..) over methods/getters/setters/index, nested cascade targets, cascades over list literals, null-aware cascades (?..)).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/cascade/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
class Logger {
  final List<String> entries = <String>[];

  String prefix = 'log';

  void write(String message) => entries.add('$prefix: $message');

  String flush() => entries.join('|');
}

class Builder {
  final List<int> items = <int>[];

  int width = 0;

  Builder add(int item) {
    items.add(item);
    return this;
  }
}

void main() {
  final logger = Logger()
    ..prefix = 'info'
    ..write('started')
    ..write('stopped');
  final log = logger.flush();

  final builder = Builder()
    ..add(1)
    ..add(2)
    ..width = 10;

  final doubled = <Logger>[Logger(), Logger()]
    ..forEach((l) => l.prefix = 'x');

  final nullLogger = nullAwareCascade();

  final nested = Logger()
    ..entries.addAll(['a', 'b'])
    ..prefix = 'n';

  var setLen = 0;
  logger
    ..entries.removeAt(0)
    ..entries.length = 1;
  setLen = logger.entries.length;

  final values = <Object?>[
    log,
    builder,
    doubled,
    nullLogger,
    nested,
    setLen,
    builder.items,
    builder.width,
    doubled.length,
  ];
  if (values.length != 9 || logger.prefix.length != 4) {
    throw StateError('unreachable');
  }
}

Logger? nullAwareCascade() {
  final Logger? maybe = Logger();
  return maybe
    ?..prefix = 'maybe'
    ..write('inside');
}
