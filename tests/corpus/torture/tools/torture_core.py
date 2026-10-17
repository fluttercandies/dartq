"""core_types family: the shared type vocabulary every other family consumes.

All files are self-contained Dart libraries in lib/. Every declaration here is
deliberately plain, valid Dart 3: the point of this package is to be the
importable foundation (points, records, tokens, mixins, enums) that the
semantic families reference across package boundaries.
"""

from __future__ import annotations

from torture_common import PKG_CORE, dart_file

FILES = [
    "geometry.dart",
    "boxes.dart",
    "store.dart",
    "comparables.dart",
    "identified.dart",
    "tokens.dart",
    "colors.dart",
    "sizes.dart",
    "marks.dart",
    "records_util.dart",
    "pairs.dart",
    "registry.dart",
    "math_ext.dart",
    "walktalk.dart",
    "nullables.dart",
    "builders.dart",
    "events.dart",
    "money.dart",
    "adder.dart",
    "legacy_defs.dart",
    "enums_mixed.dart",
    "tagged.dart",
    "heavy.dart",
    "index_calc.dart",
]


def _geometry(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Cartesian geometry used by object patterns and mixin templates.
class Point {
  final int x;
  final int y;

  const Point(this.x, this.y);

  Point.fromList(List<int> xy)
      : x = xy[0],
        y = xy[1];

  int get manhattan => x + y;
}

class Vec3 {
  final int x;
  final int y;
  final int z;

  const Vec3(this.x, this.y, this.z);

  Vec3 operator +(Vec3 o) => Vec3(x + o.x, y + o.y, z + o.z);
}
""",
    )


def _boxes(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Generic wrapper used by the generics family and constructor tear-offs.
class Box<T> {
  final T value;

  const Box(this.value);

  Box<R> map<R>(R Function(T) f) => Box<R>(f(value));
}
""",
    )


def _store(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Key-value store used as the `on` constraint of generic mixins.
class Store<T> {
  final Map<String, T> bag = {};

  void put(String key, T value) => bag[key] = value;

  T? get(String key) => bag[key];

  int get size => bag.length;
}
""",
    )


def _comparables(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Comparable box used to satisfy F-bounded instantiations.
class IBox implements Comparable<IBox> {
  final int v;

  const IBox(this.v);

  @override
  int compareTo(IBox other) => v.compareTo(other.v);
}
""",
    )


def _identified(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Interface implemented by one inheritance chain family.
abstract class Identified {
  int get id;
  String get label;
}
""",
    )


def _tokens(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Sealed token hierarchy: the cross-library exhaustiveness anchor.
sealed class Token {
  const Token();
}

final class WordT extends Token {
  final String text;

  const WordT(this.text);
}

final class NumT extends Token {
  final int n;

  const NumT(this.n);
}
""",
    )


def _colors(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Plain enum with an exhaustive switch expression member.
enum Color { red, green, blue }

extension ColorExt on Color {
  String get label => switch (this) {
        Color.red => 'red',
        Color.green => 'green',
        Color.blue => 'blue',
      };
}
""",
    )


def _sizes(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Enhanced enum with fields, methods and a static member.
enum Size {
  small(1),
  medium(2),
  large(3);

  final int rank;

  const Size(this.rank);

  bool get isBig => rank >= 3;

  static Size fromRank(int r) =>
      values.firstWhere((s) => s.rank == r, orElse: () => small);
}
""",
    )


def _marks(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Const annotation class usable as metadata.
class Mark {
  final String note;

  const Mark(this.note);
}

const Mark kAnchorMark = Mark('anchor');
""",
    )


def _records_util(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Record typedefs and record helpers.
typedef IntPair = (int, int);
typedef NamedTag = ({String tag, int id});

IntPair swap(IntPair p) => (p.$2, p.$1);

String describePair(IntPair p) => '${p.$1}/${p.$2}';

NamedTag buildTag(int id) => (tag: 't$id', id: id);
""",
    )


def _pairs(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Two-parameter generic holder.
class PairBox<A, B> {
  final A first;
  final B second;

  const PairBox(this.first, this.second);
}
""",
    )


def _registry(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Top-level mutable registry exercised by cascade and map patterns.
final Map<String, List<int>> kRegistry = <String, List<int>>{
  'a': <int>[1, 2],
  'b': <int>[3],
};
""",
    )


def _math_ext(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Extension methods on core types.
extension IntMath on int {
  int get squared => this * this;
}

extension NumListExt<T extends num> on List<T> {
  T? firstBelow(T cap) {
    for (final v in this) {
      if (v < cap) {
        return v;
      }
    }
    return null;
  }
}
""",
    )


def _walktalk(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Mixin pair used for intersection type bounds (Walk & Talk).
mixin Walk {
  int steps = 0;

  void walk() {
    steps++;
  }
}

mixin Talk {
  String greet() => 'hi';
}

class Robot with Walk, Talk {}
""",
    )


def _nullables(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Nullable helpers for flow-analysis faces.
int? maybeValue(bool b) => b ? 42 : null;

String? maybeText(bool b) => b ? 'text' : null;
""",
    )


def _builders(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Mutable builder exercised by cascade chains.
class Report {
  final List<String> rows;

  const Report(this.rows);
}

class ReportBuilder {
  final List<String> rows = <String>[];
  final Map<String, int> metrics = <String, int>{};
  String? title;

  void row(String r) => rows.add(r);

  void metric(String key, int v) => metrics[key] = v;

  ReportBuilder titled(String t) {
    title = t;
    return this;
  }

  Report build() => Report(List<String>.of(rows));
}
""",
    )


def _events(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Small class hierarchy for async payloads.
abstract class Event {
  String get name;
}

class Click extends Event {
  final int at;

  Click(this.at);

  @override
  String get name => 'click@$at';
}

class Tick extends Event {
  final int seq;

  Tick(this.seq);

  @override
  String get name => 'tick#$seq';
}
""",
    )


def _money(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Operator-overloading face (deliberately no ==/hashCode overrides).
class Money {
  final int cents;

  const Money(this.cents);

  Money operator +(Money o) => Money(cents + o.cents);

  bool operator <(Money o) => cents < o.cents;
}
""",
    )


def _adder(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Callable class (operator call).
class Adder {
  final int base;

  const Adder([this.base = 0]);

  int call(int a, int b) => base + a + b;
}
""",
    )


def _legacy_defs(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Legacy and modern typedef forms side by side.
typedef LegacyHandler = void Function(int event);
typedef void OlderHandler(int event);
typedef ModernHandler<T> = void Function(T value);
typedef PairFn = (int, int) Function(IntPair);

void runLegacy(int seed, LegacyHandler h, ModernHandler<String> ms) {
  h(seed);
  ms('$seed');
}
""",
    )


def _enums_mixed(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Enhanced enum mixing in a mixin and implementing an interface.
mixin RoleDesc {
  String get description => 'role';
}

abstract class Labeled {
  String get label;
}

enum Role with RoleDesc implements Labeled {
  admin('a'),
  user('u');

  const Role(this.label);

  @override
  final String label;
}
""",
    )


def _tagged(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Generic enhanced enum with explicitly instantiated constants.
enum Tagged<T> {
  numOne<int>(1),
  textOne<String>('one');

  final T value;

  const Tagged(this.value);
}
""",
    )


def _heavy(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Library loaded via a deferred import in the misc family.
String heavyGreeting() => 'heavy';

class Heavy {
  int cost() => 42;
}
""",
    )


def _index_calc(seed: int) -> str:
    return dart_file(
        seed,
        """
/// Tiny numeric helpers used by pattern guards.
int clampTo(int v, int lo, int hi) {
  if (v < lo) return lo;
  if (v > hi) return hi;
  return v;
}

bool inBand(int v, int lo, int hi) => v >= lo && v <= hi;
""",
    )


_EMITTER_FUNCS = [
    _geometry,
    _boxes,
    _store,
    _comparables,
    _identified,
    _tokens,
    _colors,
    _sizes,
    _marks,
    _records_util,
    _pairs,
    _registry,
    _math_ext,
    _walktalk,
    _nullables,
    _builders,
    _events,
    _money,
    _adder,
    _legacy_defs,
    _enums_mixed,
    _tagged,
    _heavy,
    _index_calc,
]

_EMITTERS = dict(zip(FILES, _EMITTER_FUNCS))


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg != PKG_CORE:
        return None
    out: dict[str, str] = {}
    for name in FILES:
        rel = f"packages/{PKG_CORE}/lib/{name}"
        out[rel] = _EMITTERS[name](seed)
    return out
