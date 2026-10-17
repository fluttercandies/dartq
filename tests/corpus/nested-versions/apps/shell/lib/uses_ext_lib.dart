// Same-dependency-two-versions face, positive half: shell consumes
// ext_lib 2.x, its declared major. The import must map to
// third_party/ext_lib/lib/ext_lib.dart — if the resolution engine
// crossed the two path-dep roots of the ext_lib family (ext_lib 2.x
// here, ext_lib_compat 1.x under packages/core), ExtGreeter and the
// Shape hierarchy would come back missing or mixed. The file also uses
// the dependency's 3.0-era patterns API from this 3.7 importer: one
// analysis run holds version 3.7 (here) and 3.0 (dependency) at the
// same time.
//
// Expectation: analyzes clean at 3.7.

import 'package:ext_lib/ext_lib.dart';

String describeShape(Shape shape) => switch (shape) {
      Circle(radius: final r) => 'circle ${r.toInt()}',
      Square(side: final s) => 'square ${s.toInt()}',
    };

void main() {
  print('${ExtGreeter.greet('uses')} ${describeShape(Square(2.5))}');
}
