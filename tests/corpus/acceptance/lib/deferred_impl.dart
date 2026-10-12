// Valid Dart 3.15 acceptance corpus — directives (companion deferred target) (deferred-import target library with top-level function and final variable (companion of directives.dart)).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/deferred/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
String deferredGreeting(String who) => 'deferred hello $who';

final List<int> deferredData = <int>[1, 2, 3];
