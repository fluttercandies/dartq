// @dart=2.12
// Below-version override face: the package default is 2.17 but this file
// pins 2.12 (override semantics anchor:
// .third/dart-sdk/pkg/front_end/lib/src/source/source_compilation_unit.dart
// :469-478 — first registered version wins).
//
// The enhanced-enum shape from pkg217/lib/main.dart is repeated here.
// Enhanced enums are a 2.17 language feature; at an applied 2.12 the
// shape rides the version gate (engine must apply the overridden version,
// not the package default).
//
// Expectation: engine resolves this file at 2.12; oracle emission for the
// below-version enhanced-enum shape follows the oracle's feature table
// and is intentionally not pinned in this corpus.

enum Tiny {
  only;

  int get n => 0;
}

void main() {
  print(Tiny.only.n);
}
