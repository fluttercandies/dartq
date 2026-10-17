"""Records + patterns family: every pattern form in its legal position.

Discipline baked into the templates:
- refutable patterns (list/map/object/relational/logical-or/cast/null-check/
  null-assert) appear only in switch expressions, switch statements, and
  if-case;
- declarations use only irrefutable record patterns;
- switch expressions over Object end with a wildcard; switch statements end
  with default — EXCEPT the sealed-Token switches, which are fully exhaustive
  (the cross-library exhaustiveness face);
- map pattern keys are always constants.
"""

from __future__ import annotations

from torture_common import PAT_PKGS, PKG_CORE, dart_file

FILES_PER_PKG = 48


def _t_record_switch(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/geometry.dart';

String classify{i}(Object o) => switch (o) {{
      (int a, int b) when a > b => 'desc pair',
      (int a, int b) => 'pair $a/$b',
      [int a, int b, ...var rest] => 'list len ${{rest.length + 2}}',
      {{'kind': String k}} => 'map kind $k',
      Point(x: int px, y: int py) when px == py => 'diagonal',
      Point(:var x) => 'point x=$x',
      String s? => 'maybe ${{s.length}}',
      1 || 2 || 3 => 'small',
      _ => 'other',
    }};

String use{i}() => classify{i}((5, 2)) + classify{i}('abc');
""",
    )


def _t_switch_stmt(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/index_calc.dart';

String band{i}(int n) {{
  switch (n) {{
    case > 90 && < 100:
      return 'hot';
    case >= 10 when n.isEven:
      return 'even mid';
    case (1 || 2 || 3):
      return 'low';
    default:
      return 'mid';
  }}
}}

String use{i}() => band{i}(clampTo(95, 0, 100));
""",
    )


def _t_if_case(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/geometry.dart';

String probe{i}(Object o) {{
  if (o case Point(x: > 0, y: < 0)) {{
    return 'quadrant';
  }} else if (o case Point(:var x, y: var y) when x + y == 0) {{
    return 'antidiagonal $x';
  }} else if (o case int n?) {{
    return 'maybe int $n';
  }} else if (o case var s! when s is String) {{
    return 'asserted ${{s.length}}';
  }}
  return 'no match';
}}

String use{i}() => probe{i}(const Point(1, -2)) + probe{i}(7);
""",
    )


def _t_declarations(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/records_util.dart';

String destructure{i}() {{
  final (a, b) = (1, 2);
  var (p, :q) = (9, q: 8);
  final IntPair swapped = swap((a, b));
  final (:tag, :id) = buildTag(3);
  return '$a/$b|$p/$q|${{swapped.$1}}|$tag-$id';
}}

String use{i}() => destructure{i}();
""",
    )


def _t_pattern_assignment(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
String rotate{i}() {{
  int a = 1;
  int b = 2;
  (a, b) = (b, a);
  return '$a$b';
}}

String use{i}() => rotate{i}();
""",
    )


def _t_list_map_stmt(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
String pick{i}(Object o) {{
  switch (o) {{
    case [var head, ...var tail] when tail.length > 1:
      return 'head $head rest ${{tail.length}}';
    case [_, _]:
      return 'exactly two';
    case {{'a': var va, 'b': var vb}}:
      return 'ab $va/$vb';
    case (String s, int n):
      return 'rec $s/$n';
    default:
      return 'none';
  }}
}}

String use{i}() => pick{i}([1, 2, 3]) + pick{i}({{'a': 1, 'b': 2}});
""",
    )


def _t_sealed_expr(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/tokens.dart';

String tok{i}(Token t) => switch (t) {{
      WordT(:final text) => text,
      NumT(n: var n) => '$n',
    }};

String use{i}() => tok{i}(const WordT('hi')) + tok{i}(const NumT(4));
""",
    )


def _t_sealed_stmt(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/tokens.dart';

String tokStmt{i}(Token t) {{
  switch (t) {{
    case WordT(:final text) when text.isNotEmpty:
      return 'w:$text';
    case NumT(:final n):
      return 'n:$n';
    default:
      return 'empty';
  }}
}}

String use{i}() => tokStmt{i}(const NumT(9));
""",
    )


def _t_for_in(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
List<(String, int)> inventory{i}() => [('gear', 3), ('bolt', 12)];

String total{i}() {{
  var t = 0;
  for (final (name, count) in inventory{i}()) {{
    t += count;
  }}
  return '$t';
}}

String use{i}() => total{i}();
""",
    )


def _t_named_records(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
String tag{i}(Object o) => switch (o) {{
      (var a, var b, tag: var t) => '$a/$b/$t',
      ({{id: int n, tag: String s}}) => 'named $n/$s',
      _ => 'none',
    }};

String use{i}() => tag{i}((1, 2, tag: 'x')) + tag{i}((id: 5, tag: 'y'));
""",
    )


def _t_relational_cast(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
String grade{i}(int n) => switch (n) {{
      >= 90 => 'a',
      >= 80 && < 90 => 'b',
      < 0 => 'neg',
      _ => 'mid',
    }};

String cast{i}(Object o) => switch (o) {{
      var n as int when n > 10 => 'big int',
      var s as String => 'str ${{s.length}}',
      _ => 'other',
    }};

String use{i}() => grade{i}(85) + cast{i}('abcd');
""",
    )


def _t_wildcards(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
String shape{i}(Object o) => switch (o) {{
      (int _, int y) => 'pair y=$y',
      [_, _, ...] => 'three plus',
      (1 || 2) && int v => 'oneOrTwo $v',
      {{'k': _}} => 'k present',
      _ => 'none',
    }};

String use{i}() => shape{i}((9, 8)) + shape{i}([1, 2, 3, 4]);
""",
    )


_TEMPLATES = (
    _t_record_switch,
    _t_switch_stmt,
    _t_if_case,
    _t_declarations,
    _t_pattern_assignment,
    _t_list_map_stmt,
    _t_sealed_expr,
    _t_sealed_stmt,
    _t_for_in,
    _t_named_records,
    _t_relational_cast,
    _t_wildcards,
)


def _wire(seed: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/tokens.dart';
import 'pat/p000.dart';
import 'pat/p006.dart';

String wire() => use0() + use6() + tokW();

String tokW() {{
  const Token t = NumT(3);
  return switch (t) {{
    WordT(:final text) => 'w$text',
    NumT(:final n) => 'n$n',
  }};
}}
""",
    )


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg not in PAT_PKGS:
        return None
    out: dict[str, str] = {}
    for i in range(FILES_PER_PKG):
        body = _TEMPLATES[i % len(_TEMPLATES)](seed, i)
        out[f"packages/{pkg}/lib/pat/p{i:03d}.dart"] = body
    out[f"packages/{pkg}/lib/wire.dart"] = _wire(seed)
    return out
