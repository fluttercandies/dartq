// Valid Dart 3.15 acceptance corpus — late/final and field initialization (late top-level/static/instance fields, late final lazily-initialized fields, final fields, initializer lists, definite assignment of late locals).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/final/, tests/language/variable/, tests/language/local_null_initialization.dart (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
late String topLevelLate;

final int topLevelFinal = compute();

int compute() => 42;

late final String lazyTopLevel = 'lazy'.toUpperCase();

class Config {
  static const int port = 8443;

  static late final String label = 'port-$port';

  final String name;

  late final String normalized = name.toLowerCase();

  late List<int> buffer;

  String? nullable;

  Config(this.name);

  Config.detuned(String raw)
      : name = raw.trim(),
        nullable = null {
    buffer = <int>[raw.length];
  }

  void prime() {
    buffer = <int>[1, 2];
    nullable = 'set';
  }

  int consume() {
    late int total;
    total = buffer.fold(0, (a, b) => a + b);
    final local = nullable ?? normalized;
    return total + local.length;
  }
}

void main() {
  topLevelLate = 't';
  final consumed = Config.detuned(' Ab ').consume();
  final config = Config('Main');
  config.prime();
  final used = config.consume();
  final items = <Object?>[
    topLevelLate,
    topLevelFinal,
    lazyTopLevel,
    Config.port,
    Config.label,
    config.normalized,
    config.buffer,
    config.nullable,
  ];
  if (consumed + used + items.length != 21) {
    throw StateError('unreachable');
  }
}
