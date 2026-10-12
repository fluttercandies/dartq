// Positive control at language version 3.6: digit separators ship at 3.6.
// Ground truth for the gate: the scanner accepts `_` in numeric literals
// unconditionally (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/scanner/
// abstract_scanner.dart:1533-1541) and the CFE version gate sits at
// pkg/front_end/lib/src/kernel/body_builder.dart:7372
// (`libraryFeatures.digitSeparators.isEnabled`; flag entry
// pkg/_fe_analyzer_shared/lib/src/experiments/flags.dart:77-80,
// enabled-by-default and expired — i.e. purely version-driven).
//
// Expectation: analyzes clean at 3.6.

const int million = 1_000_000;
const int mask = 0xFF_FF;
const double piScaled = 3.141_592;
const int small = 1_2;

void main() {
  print(million + mask + small);
  print(piScaled);
}
