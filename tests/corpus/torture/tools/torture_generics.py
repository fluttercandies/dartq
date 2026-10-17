"""Generics family: bounds, F-bounds, multi-parameter classes, nested
instantiation, generic methods, intersection bounds, typedefs (incl. record
function types), generic extensions, covariance, and generic hierarchies.

Every instantiation satisfies its bounds by construction: bounded parameters
are only instantiated with int/double/num, F-bounds with local classes that
implement Comparable of themselves, intersection bounds with the Walk&Talk
Robot from core_types.
"""

from __future__ import annotations

from torture_common import GEN_PKGS, PKG_CORE, dart_file

FILES_PER_PKG = 40


def _t_bounds(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class NumBox{i}<T extends num> {{
  final T v;

  NumBox{i}(this.v);

  num doubled() => v + v;

  T scaled(T factor) => (v + factor) as T;
}}

String use{i}() {{
  final b = NumBox{i}(2.5);
  return '${{b.doubled()}}/${{b.scaled(1.5)}}';
}}
""",
    )


def _t_fbound(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class Rank{i} implements Comparable<Rank{i}> {{
  final int stars;

  const Rank{i}(this.stars);

  @override
  int compareTo(Rank{i} other) => stars.compareTo(other.stars);
}}

class Sorted{i}<T extends Comparable<T>> {{
  final List<T> items;

  Sorted{i}(this.items);

  T? best() {{
    if (items.isEmpty) return null;
    var b = items[0];
    for (final x in items) {{
      if (x.compareTo(b) < 0) b = x;
    }}
    return b;
  }}
}}

String use{i}() {{
  final s = Sorted{i}([const Rank{i}(3), const Rank{i}(1)]);
  return '${{s.best()!.stars}}';
}}
""",
    )


def _t_registry(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class Registry{i}<K extends Object, V> {{
  final Map<K, V> _m = <K, V>{{}};

  void put(K k, V v) => _m[k] = v;

  V? get(K k) => _m[k];

  int get size => _m.length;
}}

String use{i}() {{
  final r = Registry{i}<String, int>();
  r.put('a', 1);
  return '${{r.get('a')}}/${{r.size}}';
}}
""",
    )


def _t_nested(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/boxes.dart';
import 'package:{PKG_CORE}/pairs.dart';

Map<String, List<Box<int>>> nested{i}() => {{
      'a': [const Box(1)],
      'b': [const Box(2), const Box(3)],
    }};

List<PairBox<String, int>> pairsOf{i}() => [const PairBox('x', 1)];

String use{i}() => '${{nested{i}().length}}/${{pairsOf{i}().first.second}}';
""",
    )


def _t_methods(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
T pick{i}<T extends num>(List<T> xs, int at) => xs[at];

Map<String, T> indexBy{i}<T>(Iterable<T> xs, String Function(T) key) =>
    {{for (final x in xs) key(x): x}};

String use{i}() =>
    '${{pick{i}([1, 2, 3], 1)}}/${{indexBy{i}([1, 2], (n) => 'k$n').length}}';
""",
    )


def _t_intersection(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/walktalk.dart';

T dual{i}<T extends Walk & Talk>(T x) {{
  x.walk();
  return x;
}}

String use{i}() {{
  final r = dual{i}(Robot());
  return '${{r.steps}}/${{r.greet()}}';
}}
""",
    )


def _t_typedefs(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
typedef Mapper{i}<S, T> = T Function(S);

typedef IntToString{i} = Mapper{i}<int, String>;

typedef RecFn{i} = (int, String) Function((int, int));

String conv{i}(int n) => 'n$n';

String use{i}() {{
  final IntToString{i} f = conv{i};
  final RecFn{i} g = (p) => (p.$1, 'v');
  return '${{f(3)}}/${{g((1, 2)).$2}}';
}}
""",
    )


def _t_extension(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class IBox2{i} implements Comparable<IBox2{i}> {{
  final int v;

  const IBox2{i}(this.v);

  @override
  int compareTo(IBox2{i} other) => v.compareTo(other.v);
}}

extension Best{i}<T extends Comparable<T>> on List<T> {{
  T? best() {{
    if (isEmpty) return null;
    var b = this[0];
    for (final x in this) {{
      if (x.compareTo(b) < 0) b = x;
    }}
    return b;
  }}
}}

String use{i}() =>
    '${{<IBox2{i}>[const IBox2{i}(2), const IBox2{i}(9)].best()!.v}}';
""",
    )


def _t_covariance(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class Producer{i}<T> {{
  final T value;

  Producer{i}(this.value);

  T make() => value;
}}

class Animal{i} {{}}

class Dog{i} extends Animal{i} {{}}

String use{i}() {{
  final Producer{i}<Animal{i}> p = Producer{i}(Dog{i}());
  return '${{p.make() is Dog{i}}}';
}}
""",
    )


def _t_repo(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
abstract class Repo{i}<T> {{
  T byId(String id);

  int count();
}}

class UserRec{i} {{
  final String name;

  const UserRec{i}(this.name);
}}

class UserRepo{i} extends Repo{i}<UserRec{i}> {{
  final List<UserRec{i}> _all;

  UserRepo{i}(this._all);

  @override
  UserRec{i} byId(String id) => _all.firstWhere((u) => u.name == id);

  @override
  int count() => _all.length;
}}

String use{i}() {{
  final r = UserRepo{i}([const UserRec{i}('ann'), const UserRec{i}('bob')]);
  return '${{r.byId('ann').name}}/${{r.count()}}';
}}
""",
    )


def _t_bounded_pair(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class PairN{i}<A extends num, B extends A> {{
  final A a;
  final B b;

  PairN{i}(this.a, this.b);

  num sum() => a + b;
}}

String use{i}() =>
    '${{PairN{i}(1, 2).sum()}}/${{PairN{i}(1.5, 2.5).sum()}}';
""",
    )


def _t_boxed_constraint(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/boxes.dart';

class Unbox{i}<T> {{
  final Box<T> box;

  const Unbox{i}(this.box);

  R mapBox<R>(R Function(T) f) => box.map(f).value;
}}

String use{i}() {{
  const u = Unbox{i}(Box(3));
  return '${{u.mapBox((n) => n * 2)}}';
}}
""",
    )


_TEMPLATES = (
    _t_bounds,
    _t_fbound,
    _t_registry,
    _t_nested,
    _t_methods,
    _t_intersection,
    _t_typedefs,
    _t_extension,
    _t_covariance,
    _t_repo,
    _t_bounded_pair,
    _t_boxed_constraint,
)


def _wire(seed: int) -> str:
    return dart_file(
        seed,
        """
import 'gen/g000.dart';
import 'gen/g001.dart';

String wire() => use0() + use1();
""",
    )


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg not in GEN_PKGS:
        return None
    out: dict[str, str] = {}
    for i in range(FILES_PER_PKG):
        body = _TEMPLATES[i % len(_TEMPLATES)](seed, i)
        out[f"packages/{pkg}/lib/gen/g{i:03d}.dart"] = body
    out[f"packages/{pkg}/lib/wire.dart"] = _wire(seed)
    return out
