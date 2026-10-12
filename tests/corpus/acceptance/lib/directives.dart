// Valid Dart 3.15 acceptance corpus — directives (library directive, import with `deferred as`, import with `as` + hide combinator, export with show combinator, export of core library symbols, part directive).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/library/, tests/language/part/, tests/language/import/, tests/language/prefix/, tests/language/deferred/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
library;

import 'deferred_impl.dart' deferred as lazy;
import 'deferred_impl.dart' as plain hide deferredData;
export 'deferred_impl.dart' show deferredGreeting;
export 'dart:core' show Duration;
part 'directives_part.dart';

Future<String> useDeferred() async {
  await lazy.loadLibrary();
  return lazy.deferredGreeting('late');
}

void main() {
  final stamped = plain.deferredGreeting('now');
  final duration = Duration(seconds: 1);
  final partValue = _partHelper('x');
  final table = partTable;
  final pending = useDeferred();
  final items = <Object?>[stamped, duration, partValue, table, pending];
  if (items.length != 5) {
    throw StateError('unreachable');
  }
}
