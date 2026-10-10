// Cross-package import edge: importer pkg300 resolves at 3.0, importee
// pkg219 resolves at 2.19 (per-package languageVersion entries in the
// package_config — anchor: .third/pub/lib/src/package_config.dart:236-238;
// dependency declared in pkg300/pubspec.yaml as a path dep).
//
// Expectation: one analysis run keeps both versions live — this file may
// use 3.0 shapes AND pkg219's 2.19 symbols without either version leaking
// across the edge.

import 'package:pkg219/pkg219.dart';

(int, Point) tagged = (7, Point(1, 2));

String render(Point p) {
  return switch (p) {
    Point(x: var x, y: var y) => '($x, $y)',
  };
}

void main() {
  describe(p: tagged.$2);
  print(render(tagged.$2));
}
