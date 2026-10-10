// Positive control at language version 3.3: extension types ship at 3.3
// and are legal at the package default. Feature set derives from the
// library version (anchor: .third/dart-sdk/pkg/front_end/lib/src/source/
// source_compilation_unit.dart:348-353; representation declaration
// parsing in the shared parser,
// pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart, with the
// expected-representation messages in
// pkg/front_end/lib/src/codes/diagnostic.g.dart).
//
// Expectation: analyzes clean at 3.3.

extension type Meters(int value) {
  Meters get doubled => Meters(value * 2);
}

extension type Id(String raw) implements Object {
  bool get isEmpty => raw.isEmpty;
}

void main() {
  final m = Meters(3);
  final id = Id('a-1');
  print(m.doubled.value + id.raw.length);
}
