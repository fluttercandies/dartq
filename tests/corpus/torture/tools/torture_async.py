"""Async family: await-in-loop gaps, async*/sync* generators, yield*
delegation, await-for (incl. record streams), try/on/finally interleavings,
Completer, Future.wait, switch over an awaited value, stream combinators,
and pattern-based flow after await.

Every template exposes `Future<String> run<i>()`; the package wire file
exposes both a sync `wire()` (uniform app contract) and an async
`wireAsync()` that actually awaits two runs.
"""

from __future__ import annotations

from torture_common import ASYNC_PKGS, dart_file

FILES_PER_PKG = 24


def _t_await_loop(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Future<String> run{i}() async {{
  var t = 0;
  try {{
    for (var k = 0; k < 3; k++) {{
      t += await Future<int>.value(k);
    }}
  }} on Exception {{
    t = -1;
  }} finally {{
    t += 10;
  }}
  return '$t';
}}
""",
    )


def _t_await_for(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Stream<int> gen{i}(int n) async* {{
  for (var k = 1; k <= n; k++) {{
    yield k;
  }}
}}

Future<String> run{i}() async {{
  var t = 0;
  await for (final v in gen{i}(4)) {{
    t += v;
  }}
  return '$t';
}}
""",
    )


def _t_sync_star(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Iterable<int> base{i}(int n) sync* {{
  for (var k = 0; k < n; k++) {{
    yield k * 2;
  }}
}}

Iterable<int> chain{i}(int n) sync* {{
  yield* base{i}(n);
  yield 99;
}}

Future<String> run{i}() async {{
  var t = 0;
  for (final v in chain{i}(3)) {{
    t += v;
  }}
  return '$t';
}}
""",
    )


def _t_completer(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'dart:async';

Future<String> run{i}() async {{
  final c = Completer<int>();
  c.complete(7);
  final v = await c.future;
  return '$v';
}}
""",
    )


def _t_future_wait(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Future<int> one{i}() async => 1;

Future<int> two{i}() async => 2;

Future<String> run{i}() async {{
  final rs = await Future.wait<int>([one{i}(), two{i}()]);
  return '${{rs[0] + rs[1]}}';
}}
""",
    )


def _t_switch_await(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Future<int> fetch{i}(int k) async => k * 10;

Future<String> run{i}() async {{
  switch (await fetch{i}(2)) {{
    case 20:
      return 'twenty';
    case 30:
      return 'thirty';
    default:
      return 'other';
  }}
}}
""",
    )


def _t_combinators(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'dart:async';

Future<String> run{i}() async {{
  final s = Stream<int>.fromIterable(const [1, 2, 3, 4]);
  final taken = s.where((x) => x.isEven).map((x) => x * 10).take(2);
  var t = 0;
  await for (final v in taken) {{
    t += v;
  }}
  return '$t';
}}
""",
    )


def _t_record_stream(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Stream<(int, int)> pairs{i}() async* {{
  yield (1, 2);
  yield (3, 4);
}}

Future<String> run{i}() async {{
  var t = 0;
  await for (final (a, b) in pairs{i}()) {{
    t += a * b;
  }}
  return '$t';
}}
""",
    )


def _t_rethrow(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Future<int> boom{i}() async {{
  throw StateError('boom');
}}

Future<String> run{i}() async {{
  try {{
    await boom{i}();
    return 'no';
  }} on StateError {{
    return 'caught';
  }}
}}
""",
    )


def _t_delayed(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Future<String> run{i}() async {{
  await Future<void>.delayed(const Duration(milliseconds: 1));
  return 'done';
}}
""",
    )


def _t_pattern_flow(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Future<String> run{i}() async {{
  final Object v = await Future<Object>.value(42);
  if (v case final int n when n.isEven) {{
    return 'even $n';
  }}
  return 'odd-ish';
}}
""",
    )


def _t_nullable_await(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
Future<int?> maybeLater{i}(bool b) async => b ? 5 : null;

Future<String> run{i}() async {{
  final v = await maybeLater{i}(true);
  return '${{v ?? 0}}';
}}
""",
    )


_TEMPLATES = (
    _t_await_loop,
    _t_await_for,
    _t_sync_star,
    _t_completer,
    _t_future_wait,
    _t_switch_await,
    _t_combinators,
    _t_record_stream,
    _t_rethrow,
    _t_delayed,
    _t_pattern_flow,
    _t_nullable_await,
)


def _wire(seed: int) -> str:
    return dart_file(
        seed,
        """
import 'asyncx/a000.dart';
import 'asyncx/a001.dart';

Future<String> wireAsync() async => '${await run0()}|${await run1()}';

String wire() => 'async';
""",
    )


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg not in ASYNC_PKGS:
        return None
    out: dict[str, str] = {}
    for i in range(FILES_PER_PKG):
        body = _TEMPLATES[i % len(_TEMPLATES)](seed, i)
        out[f"packages/{pkg}/lib/asyncx/a{i:03d}.dart"] = body
    out[f"packages/{pkg}/lib/wire.dart"] = _wire(seed)
    return out
