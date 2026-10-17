// Same-dependency-two-versions face, positive half for the other major:
// core consumes ext_lib_compat 1.x (its declared dependency), while
// apps/shell holds ext_lib 2.x. The import must map to
// third_party/ext_lib_compat/lib/ext_lib_compat.dart — a resolution
// engine that crossed the two roots of the family would surface
// ext_lib's symbols under this name. A 3.6 importer against a 2.19
// dependency: two settled versions live across one import edge.
//
// Expectation: analyzes clean at 3.6.

import 'package:ext_lib_compat/ext_lib_compat.dart';

String compatBanner() {
  final buffer = Buffer();
  buffer.add(7);
  return '${CompatGreeter.greetLegacy('core')} ${buffer.length}';
}

void main() {
  print(compatBanner());
}
