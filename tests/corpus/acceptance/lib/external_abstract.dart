// Valid Dart 3.15 acceptance corpus — external/abstract members (external top-level functions and accessors, abstract fields with implicit accessors, abstract classes with unimplemented members, external constructors and static methods).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/external_abstract_fields/, tests/language/abstract/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
external String platformName();

external int get platformCount;

external set platformCount(int value);

external void configure({String? option});

abstract class Channel {
  String get name;

  int send(List<int> bytes);

  void close();

  bool _debug = false;

  bool get debug => _debug;
}

class FileChannel implements Channel {
  @override
  final String name;

  FileChannel(this.name);

  @override
  int send(List<int> bytes) => bytes.length;

  @override
  void close() {}

  @override
  bool _debug = false;

  @override
  bool get debug => _debug;
}

abstract class Adapter {
  external factory Adapter(String id);

  external void bind(Object handle);

  external static Adapter? lookup(String id);

  String describe() => 'adapter';
}

class Width {
  external Width.fromRatio(double ratio);

  late final int px = 10;
}

void main() {
  final channel = FileChannel('file');
  final bytes = <int>[1, 2, 3];
  final sent = channel.send(bytes);
  channel.close();
  final adapters = <Object>[channel, channel.name, sent, channel.debug];
  final describe = Adapter;
  final w = Width.fromRatio(0.5);
  if (adapters.length != 4 || w.px != 10 || describe.toString().isEmpty) {
    throw StateError('unreachable');
  }
}
