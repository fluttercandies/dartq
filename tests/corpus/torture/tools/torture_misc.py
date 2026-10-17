"""Misc family: cascades (incl. null-aware), tear-offs (constructor, getter,
generic), legacy + modern typedefs, basic/advanced/generic enhanced enums,
part/part-of pairs, conditional-import facades, flow-sensitive nullability,
deferred imports, and metadata/operator/callable faces.

Template functions return a dict of {file suffix: body} because parts and
conditional-import sites emit several mutually-referencing files at once.
"""

from __future__ import annotations

from torture_common import MISC_PKGS, PKG_CORE, dart_file

SITES_PER_PKG = 40


def _t_cascades(seed: int, i: int) -> dict[str, str]:
    body = f"""
import 'package:{PKG_CORE}/builders.dart';

String use{i}() {{
  final rb = ReportBuilder()
    ..row('r1')
    ..row('r2')
    ..metric('m', 1)
    ..title = 'T';
  final rep = rb.build();
  return '${{rep.rows.length}}/${{rb.metrics.length}}';
}}
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_null_cascade(seed: int, i: int) -> dict[str, str]:
    body = f"""
import 'package:{PKG_CORE}/builders.dart';

String use{i}() {{
  ReportBuilder? maybe;
  maybe?..row('a')..row('b');
  maybe = ReportBuilder()..row('c');
  return '${{maybe.rows.length}}';
}}
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_tearoffs(seed: int, i: int) -> dict[str, str]:
    body = f"""
import 'package:{PKG_CORE}/boxes.dart';
import 'package:{PKG_CORE}/geometry.dart';

const Point kPt{i} = Point(2, 3);

String use{i}() {{
  final ctor = Point.new;
  final fromList = Point.fromList;
  final getter = kPt{i}.manhattan;
  final boxed = Box<int>.new;
  return '${{ctor(1, 2).x}}/${{fromList([4, 5]).y}}/$getter/${{boxed(9).value}}';
}}
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_typedefs(seed: int, i: int) -> dict[str, str]:
    body = f"""
typedef Legacy{i} = void Function(int);

typedef void Older{i}(int event);

typedef Modern{i}<T> = T Function(T);

String use{i}() {{
  Modern{i}<int> dbl = (n) => n * 2;
  var out = 0;
  void collect(int x) {{
    out += x;
  }}

  final Legacy{i} h = collect;
  for (final v in const [1, 2]) {{
    h(v);
  }}
  return '${{dbl(3)}}/$out';
}}
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_enum_basic(seed: int, i: int) -> dict[str, str]:
    body = f"""
enum Flavor{i} {{
  sweet(1),
  salty(2);

  final int code;

  const Flavor{i}(this.code);

  bool get isSweet => this == Flavor{i}.sweet;
}}

String use{i}() =>
    '${{Flavor{i}.values.length}}/${{Flavor{i}.salty.code}}/${{Flavor{i}.sweet.isSweet}}';
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_enum_advanced(seed: int, i: int) -> dict[str, str]:
    body = f"""
mixin Marked{i} {{
  String get marked => 'm';
}}

abstract class Described{i} {{
  String get describe;
}}

enum Level{i} with Marked{i} implements Described{i} {{
  low(1),
  high(2);

  const Level{i}(this.describe);

  @override
  final String describe;

  static Level{i} top() => high;
}}

String use{i}() => '${{Level{i}.high.describe}}/${{Level{i}.top().marked}}';
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_enum_generic(seed: int, i: int) -> dict[str, str]:
    body = f"""
enum Slot{i}<T> {{
  intSlot<int>(1),
  strSlot<String>('s');

  final T value;

  const Slot{i}(this.value);

  T get again => value;
}}

String use{i}() => '${{Slot{i}.intSlot.value}}/${{Slot{i}.strSlot.again}}';
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_parts(seed: int, i: int) -> dict[str, str]:
    host = f"""
import 'package:{PKG_CORE}/geometry.dart';

part 'part_impl_{i}.dart';

String hostCall{i}() => '${{_helper(const Point(1, 2))}}/$_counter';

class HostVisible{i} {{
  int get visible => _counter;
}}

String use{i}() => hostCall{i}();
"""
    part = f"""
part of 'm{i:03d}.dart';

int _helper(Point p) => p.x + p.y;

int _counter = 7;
"""
    return {f"m{i:03d}.dart": dart_file(seed, host), f"part_impl_{i}.dart": dart_file(seed, part)}


def _t_conditional(seed: int, i: int) -> dict[str, str]:
    facade = f"""
export 'net_stub_{i}.dart'
    if (dart.library.io) 'net_vm_{i}.dart'
    if (dart.library.js) 'net_web_{i}.dart';
"""
    stub = f"""
String netName{i}() => 'stub';

int netCost{i}() => 0;
"""
    vm = f"""
String netName{i}() => 'vm';

int netCost{i}() => 1;
"""
    web = f"""
String netName{i}() => 'web';

int netCost{i}() => 2;
"""
    use = f"""
import 'm{i:03d}.dart';

String use{i}() => '${{netName{i}()}}/${{netCost{i}()}}';
"""
    return {
        f"m{i:03d}.dart": dart_file(seed, facade),
        f"net_stub_{i}.dart": dart_file(seed, stub),
        f"net_vm_{i}.dart": dart_file(seed, vm),
        f"net_web_{i}.dart": dart_file(seed, web),
        f"net_use_{i}.dart": dart_file(seed, use),
    }


def _t_nullability(seed: int, i: int) -> dict[str, str]:
    body = f"""
int? maybe{i}(bool b) => b ? 7 : null;

String flow{i}(bool b) {{
  final v = maybe{i}(b);
  if (v == null) {{
    return 'zero';
  }}
  return '${{v + 1}}';
}}

String definite{i}(bool b) {{
  late final int x;
  if (b) {{
    x = 1;
  }} else {{
    x = 2;
  }}
  return '$x';
}}

String use{i}() => flow{i}(true) + definite{i}(false) + '${{maybe{i}(false) ?? 0}}';
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_deferred(seed: int, i: int) -> dict[str, str]:
    body = f"""
import 'package:{PKG_CORE}/heavy.dart' deferred as heavy{i};

Future<String> loadHeavy{i}() async {{
  await heavy{i}.loadLibrary();
  return '${{heavy{i}.heavyGreeting()}}/${{heavy{i}.Heavy().cost()}}';
}}

String use{i}() => 'deferred';
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


def _t_extras(seed: int, i: int) -> dict[str, str]:
    body = f"""
import 'package:{PKG_CORE}/adder.dart';
import 'package:{PKG_CORE}/marks.dart';
import 'package:{PKG_CORE}/money.dart';

@Mark('hot')
int work{i}(int n) => n * 2;

String use{i}() {{
  const sum = Adder(10);
  final m = const Money(5) + const Money(7);
  final cheaper = const Money(1) < m;
  return '${{work{i}(3)}}/${{sum(1, 2)}}/$m/$cheaper';
}}
"""
    return {f"m{i:03d}.dart": dart_file(seed, body)}


_TEMPLATES = (
    _t_cascades,
    _t_null_cascade,
    _t_tearoffs,
    _t_typedefs,
    _t_enum_basic,
    _t_enum_advanced,
    _t_enum_generic,
    _t_parts,
    _t_conditional,
    _t_nullability,
    _t_deferred,
    _t_extras,
)


def _wire(seed: int) -> str:
    return dart_file(
        seed,
        """
import 'misc/m000.dart';
import 'misc/m001.dart';

String wire() => use0() + use1();
""",
    )


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg not in MISC_PKGS:
        return None
    out: dict[str, str] = {}
    for i in range(SITES_PER_PKG):
        for suffix, body in _TEMPLATES[i % len(_TEMPLATES)](seed, i).items():
            out[f"packages/{pkg}/lib/misc/{suffix}"] = body
    out[f"packages/{pkg}/lib/wire.dart"] = _wire(seed)
    return out
