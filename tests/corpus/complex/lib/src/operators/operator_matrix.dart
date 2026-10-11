/// Operator overload matrix over warehouse geometry and bitmask flags:
/// arithmetic, comparison, index, bitwise (including triple-shift), unary
/// and the call operator.
library;

import 'dart:math' as math;

/// 2D floor-plan vector with the full arithmetic/comparison surface.
class Vec2 implements Comparable<Vec2> {
  final int x;
  final int y;

  const Vec2(this.x, this.y);

  static const Vec2 origin = Vec2(0, 0);

  Vec2 operator +(Vec2 other) => Vec2(x + other.x, y + other.y);
  Vec2 operator -(Vec2 other) => Vec2(x - other.x, y - other.y);
  Vec2 operator -() => Vec2(-x, -y);
  Vec2 operator *(int scale) => Vec2(x * scale, y * scale);

  @override
  int compareTo(Vec2 other) {
    final byX = x.compareTo(other.x);
    return byX != 0 ? byX : y.compareTo(other.y);
  }

  bool operator <(Vec2 other) => compareTo(other) < 0;
  bool operator <=(Vec2 other) => compareTo(other) <= 0;
  bool operator >(Vec2 other) => compareTo(other) > 0;
  bool operator >=(Vec2 other) => compareTo(other) >= 0;

  @override
  bool operator ==(Object other) =>
      other is Vec2 && other.x == x && other.y == y;

  @override
  int get hashCode => Object.hash(x, y);

  @override
  String toString() => 'Vec2($x, $y)';
}

/// Dense shelf grid addressed by coordinates; index read/write pair.
class ShelfGrid<T extends Object> {
  final int width;
  final int height;
  final Map<Vec2, T> _cells = {};

  ShelfGrid({required this.width, required this.height});

  bool inBounds(Vec2 at) =>
      at.x >= 0 && at.x < width && at.y >= 0 && at.y < height;

  T? operator [](Vec2 at) => inBounds(at) ? _cells[at] : null;

  void operator []=(Vec2 at, T value) {
    if (inBounds(at)) {
      _cells[at] = value;
    }
  }

  int get occupied => _cells.length;
}

/// Capacity flags packed into one word: bitwise family including `>>>`,
/// enabled since the triple-shift release.
class CapacityFlags {
  final int bits;

  const CapacityFlags(this.bits);

  static const CapacityFlags none = CapacityFlags(0);
  static const CapacityFlags chilled = CapacityFlags(1 << 0);
  static const CapacityFlags hazmat = CapacityFlags(1 << 1);
  static const CapacityFlags oversized = CapacityFlags(1 << 2);

  bool get isChilled => bits & chilled.bits != 0;

  CapacityFlags operator |(CapacityFlags other) =>
      CapacityFlags(bits | other.bits);

  CapacityFlags operator &(CapacityFlags other) =>
      CapacityFlags(bits & other.bits);

  CapacityFlags operator ^(CapacityFlags other) =>
      CapacityFlags(bits ^ other.bits);

  CapacityFlags operator ~() => CapacityFlags(~bits);

  CapacityFlags operator <<(int shift) => CapacityFlags(bits << shift);

  CapacityFlags operator >>(int shift) => CapacityFlags(bits >> shift);

  CapacityFlags operator >>>(int shift) => CapacityFlags(bits >>> shift);

  /// Integer division and modulo for slot math.
  int slotsFor(int slotsPerBay) => bits ~/ slotsPerBay % slotsPerBay;

  @override
  String toString() => 'Flags(${bits.toRadixString(2)})';
}

/// A callable dispatch object: [call] lets instances be invoked like
/// functions, the shape gateways use for pluggable normalization.
class Normalizer {
  final math.Random _random;

  Normalizer(this._random);

  String call(String input, {int fallbackLength = 8}) {
    final trimmed = input.trim();
    if (trimmed.length >= fallbackLength) {
      return trimmed;
    }
    return trimmed.padRight(fallbackLength, '_');
  }

  int noise() => _random.nextInt(100);
}
