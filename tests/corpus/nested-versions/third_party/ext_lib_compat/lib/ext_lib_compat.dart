// Vendored ext_lib_compat 1.x ("ext_lib major 1" family) at language
// version 2.19. API surface deliberately distinct from ext_lib 2.x —
// the two majors of the dependency family must never contaminate each
// other's package scope (see apps/shell/lib/uses_ext_lib.dart and
// packages/core/lib/uses_compat.dart for the positive halves).
//
// Expectation: analyzes clean at 2.19 (pre-records syntax only).

class CompatGreeter {
  static String greetLegacy(String who) => 'compat/$who';
}

class Buffer {
  final List<int> bytes;

  Buffer() : bytes = <int>[];

  void add(int byte) => bytes.add(byte);

  int get length => bytes.length;
}

void main() {
  final b = Buffer();
  b.add(1);
  print('${CompatGreeter.greetLegacy('main')} ${b.length}');
}
