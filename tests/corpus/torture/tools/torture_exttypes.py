"""Extension-type family: representation types (int/String/List/nullable/
generic), operators, statics, redirecting constructors, and Comparable
implementations.

Every template is self-contained and instantiates extension types without
const (extension-type const-ness is deliberately not exercised).
"""

from __future__ import annotations

from torture_common import EXTT_PKGS, PKG_CORE, dart_file

FILES_PER_PKG = 8 * 5  # 40 files per package


def _t_int_rep(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
extension type Meters{i}(int value) {{
  Meters{i} operator +(Meters{i} o) => Meters{i}(value + o.value);

  int get raw => value;

  bool get isPositive => value > 0;

  static Meters{i} zero() => Meters{i}(0);
}}

String use{i}() {{
  final m = Meters{i}(3) + Meters{i}.zero();
  return '${{m.raw}}/${{m.isPositive}}';
}}
""",
    )


def _t_string_rep(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
extension type UserId{i}(String id) {{
  String get domain => id.contains(':') ? id.split(':')[0] : id;

  int get chars => id.length;
}}

String use{i}() => '${{UserId{i}('acme:u{i}').domain}}/${{UserId{i}('x').chars}}';
""",
    )


def _t_list_rep(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/geometry.dart';

extension type Points{i}(List<Point> ps) {{
  int totalX() {{
    var t = 0;
    for (final p in ps) {{
      t += p.x;
    }}
    return t;
  }}

  int get count => ps.length;
}}

String use{i}() {{
  final pts = Points{i}([const Point(1, 2), const Point(3, 4)]);
  return '${{pts.count}}/${{pts.totalX()}}';
}}
""",
    )


def _t_generic_rep(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
extension type Crate{i}<T>(List<T> items) {{
  T get first => items.first;

  int get count => items.length;

  Crate{i}<R> map<R>(R Function(T) f) =>
      Crate{i}<R>([for (final x in items) f(x)]);
}}

String use{i}() {{
  final c = Crate{i}(<int>[1, 2, 3]);
  final s = c.map((x) => x * 2);
  return '${{c.count}}/${{s.first}}';
}}
""",
    )


def _t_nullable_rep(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
extension type Loose{i}(int? v) {{
  bool get present => v != null;

  int orZero() => v ?? 0;
}}

String use{i}() => '${{Loose{i}(null).present}}/${{Loose{i}(5).orZero()}}';
""",
    )


def _t_comparable(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
extension type Score{i}(int raw) implements Comparable<Score{i}> {{
  @override
  int compareTo(Score{i} other) => raw.compareTo(other.raw);
}}

String use{i}() {{
  final a = Score{i}(2);
  final b = Score{i}(5);
  return '${{a.compareTo(b)}}';
}}
""",
    )


def _t_redirect(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
extension type Seconds{i}(int v) {{
  Seconds{i}.fromMinutes(int m) : this(m * 60);
}}

String use{i}() => '${{Seconds{i}.fromMinutes(2).v}}';
""",
    )


def _t_statics(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
extension type Handle{i}(int n) {{
  static final Handle{i} root = Handle{i}(0);

  static Handle{i} of(int n) => Handle{i}(n < 0 ? 0 : n);

  Handle{i} get next => Handle{i}(n + 1);
}}

String use{i}() {{
  final xs = <Handle{i}>[Handle{i}.root, Handle{i}.of(2)];
  return '${{xs.length}}/${{xs[1].next.n}}';
}}
""",
    )


_TEMPLATES = (
    _t_int_rep,
    _t_string_rep,
    _t_list_rep,
    _t_generic_rep,
    _t_nullable_rep,
    _t_comparable,
    _t_redirect,
    _t_statics,
)


def _wire(seed: int) -> str:
    return dart_file(
        seed,
        """
import 'ext/e000.dart';

String wire() => use0();
""",
    )


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg not in EXTT_PKGS:
        return None
    out: dict[str, str] = {}
    for i in range(FILES_PER_PKG):
        body = _TEMPLATES[i % len(_TEMPLATES)](seed, i)
        out[f"packages/{pkg}/lib/ext/e{i:03d}.dart"] = body
    out[f"packages/{pkg}/lib/wire.dart"] = _wire(seed)
    return out
