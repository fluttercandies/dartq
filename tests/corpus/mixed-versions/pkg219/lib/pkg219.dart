// Pre-records baseline at language version 2.19 (package default). No
// records, no patterns, no class modifiers — plain 2.12-era shapes only.
// This is the importee of pkg300/lib/uses_pkg219.dart: importer 3.0,
// importee 2.19 (cross-package version edge).
//
// Expectation: analyzes clean at 2.19.

class Point {
  final int x;
  final int y;

  const Point(this.x, this.y);

  int get sum => x + y;
}

void describe({required Point p}) {
  print('${p.x}:${p.y} sum=${p.sum}');
}

void main() {
  describe(p: const Point(2, 3));
}
