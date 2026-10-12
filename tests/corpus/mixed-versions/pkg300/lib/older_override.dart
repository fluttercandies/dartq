// @dart=2.19
// Below-version override face: the package default is 3.0 but this file
// pins 2.19 (override semantics anchor:
// upstream Dart SDK source, pkg/front_end/lib/src/source/source_compilation_unit.dart
// :469-478). The same record/pattern/sealed shapes as pkg300/lib/main.dart
// are repeated here at an applied 2.19.
//
// Expectation: the engine must apply 2.19 to this file; the mirror face —
// the same shapes at an applied 3.0 in pkg219/lib/future_override.dart —
// must stay clean. Oracle emission for the below-version shapes follows
// the oracle's feature table and is intentionally not pinned here.

(int, int) pair = (1, 2);

sealed class Shape {}

class Dot extends Shape {}

void main() {
  print(pair);
  print(Dot());
}
