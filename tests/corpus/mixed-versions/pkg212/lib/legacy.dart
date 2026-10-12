// @dart=2.9
// Language-version override BELOW the package default (2.12): per
// upstream Dart SDK source, pkg/front_end/lib/src/source/source_compilation_unit.dart
// :469-478, a source-level annotation replaces the package_config default,
// and only the first registered version applies. Scanner recognition:
// upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/scanner/abstract_scanner.dart
// :1837 and token.dart:809.
//
// Legacy (pre-null-safety) mode face: with the non-nullable feature off,
// `int?` is outside this file's grammar and `int y = null;` is legal —
// the exact reverse of pkg212/lib/main.dart at 2.12.
//
// Expectation: the engine must apply 2.9 to this file (below-version
// gating face); `int y = null;` stays clean here and the `int?` shape
// rides the version gate.

int? x;

int y = null;

class Holder {
  var value = null;
}

void main() {
  y = Holder().value ?? 0;
}
