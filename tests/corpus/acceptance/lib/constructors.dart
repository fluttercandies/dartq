// Valid Dart 3.15 acceptance corpus — constructors (const/initializing-formal constructors, initializer lists with asserts, optional/required named parameters, factory and redirecting-factory constructors, const factory redirect, super parameters, redirecting generative constructors).
//
// Provenance: shapes modeled on the official language tests
// .third/dart-sdk/tests/language/constructor/, tests/language/factory/, tests/language/redirecting/, tests/language/initializing_formal/ (SDK main @ e6f4207a). Oracle-verified
// 2026-10-12: zero rows under fvm dart 3.13.1 `dart analyze --format=machine`
// in a probe package (`environment: sdk: ^3.13.0`). Covered by
// scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq check` machine face).
class Point {
  final int x;
  final int y;

  const Point(this.x, this.y);

  const Point.origin() : x = 0, y = 0;

  Point.relocate({required int left, int top = 0, int? depth})
      : x = left,
        y = top + (depth ?? 0) {
    assert(x >= 0 && y >= 0, 'negative point');
  }

  factory Point.polar(int r, [int theta = 1]) {
    return Point(r, theta);
  }

  factory Point.json(Map<String, Object?> json) = _JsonPoint;

  const factory Point.unit() = _UnitPoint;

  Point mirrored() => Point(y, x);
}

class _UnitPoint implements Point {
  const _UnitPoint();

  @override
  int get x => 1;

  @override
  int get y => 1;

  @override
  Point mirrored() => const Point.origin();
}

class Vec3 extends Point {
  final int z;

  Vec3(super.x, super.y, {this.z = 0});

  Vec3.flattened(Point p, int z) : this(p.x, p.y, z: z);

  Vec3.throughJson(Point p) : z = 0, super(p.x, p.y);

  @override
  Point mirrored() => Vec3(y, x, z: z);
}

class _JsonPoint extends Point {
  _JsonPoint(Map<String, Object?> json)
      : super.relocate(left: (json['left'] as int?) ?? 0);
}

void main() {
  const unit = Point.unit();
  final origin = const Point.origin();
  final relocated = Point.relocate(left: 2, top: 3, depth: 1);
  final polar = Point.polar(5);
  final fromJson = Point.json({'left': 1});
  final v = Vec3(1, 2, z: 3);
  final flattened = Vec3.flattened(relocated, 7);
  final through = Vec3.throughJson(relocated);
  final points = <Point>[unit, origin, relocated, polar, fromJson, v, flattened, through];
  var sum = 0;
  for (final p in points) {
    final m = p.mirrored();
    sum += m.x + m.y;
  }
  if (sum != 27) {
    throw StateError('unreachable');
  }
}
