// Valid Dart 3.15 acceptance corpus — directives (companion part) (part-of unit with string URI, library-private declarations shared with the containing library (companion of directives.dart)).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/part/, tests/language/library/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
part of 'directives.dart';

String _partHelper(String suffix) => 'from-part-$suffix';

final Map<String, int> partTable = <String, int>{'one': 1};
