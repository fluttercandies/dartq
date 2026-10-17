"""Mixin application family: `on` constraints, application order, generic
mixins, super-call overrides, mixin + implements, and cross-package mixin
applications (mix_00 publishes provider mixins that mix_01..03 apply).

Every template is a self-contained valid Dart library: the `on` constraint of
each mixin is satisfied by the superclass chain of every application emitted
next to it, and every `use_<id>()` function is normalized to return String so
the per-package wire file can call them blindly.
"""

from __future__ import annotations

from torture_common import MIX_PKGS, PKG_CORE, dart_file

FILES_PER_PKG = 56
PROVIDERS = 10
SELF_TEMPLATES = ("simple", "chain2", "generic", "iface", "pointext", "store", "trace")


def _t_simple(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class MB{i} {{
  int op{i}(int x) => x + {i % 7};

  int get val => {i % 5};
}}

mixin MM{i} on MB{i} {{
  int extra() => op{i}(2);
}}

class MA{i} extends MB{i} with MM{i} {{}}

String use{i}() => 'v=${{MA{i}().extra()}}';
""",
    )


def _t_chain2(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class MC{i} {{
  void core() {{}}
}}

mixin M1_{i} on MC{i} {{
  int a() => 1;
}}

mixin M2_{i} on MC{i} {{
  int b() => 2;
}}

class MT{i} extends MC{i} with M1_{i}, M2_{i} {{}}

String use{i}() => '${{MT{i}().a() + MT{i}().b()}}';
""",
    )


def _t_generic(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class MG{i}<T> {{
  final Map<String, T> bag = <String, T>{{}};

  void put(String k, T v) => bag[k] = v;

  T? get(String k) => bag[k];
}}

mixin MBuf{i}<T> on MG{i}<T> {{
  final List<String> seen = <String>[];

  @override
  void put(String k, T v) {{
    seen.add(k);
    super.put(k, v);
  }}

  List<String> get seenKeys => List<String>.unmodifiable(seen);
}}

class MFast{i} extends MG{i}<int> with MBuf{i}<int> {{}}

String use{i}() {{
  final c = MFast{i}();
  c.put('a', 1);
  c.put('b', 2);
  return '${{c.seenKeys.length}}';
}}
""",
    )


def _t_iface(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
abstract class MI{i} {{
  void doit();
}}

class MB2{i} {{}}

mixin MImp{i} on MB2{i} implements MI{i} {{
  @override
  void doit() {{}}
}}

class MU{i} extends MB2{i} with MImp{i} {{}}

String use{i}() => '${{MU{i}() is MI{i}}}';
""",
    )


def _t_pointext(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/geometry.dart';

mixin MPt{i} on Point {{
  int get sum => x + y;
}}

class PApp{i} extends Point with MPt{i} {{
  const PApp{i}(super.x, super.y);
}}

String use{i}() => '${{const PApp{i}(3, 4).sum}}';
""",
    )


def _t_store(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:{PKG_CORE}/store.dart';

mixin MStore{i}<T> on Store<T> {{
  final Map<String, T> _memo = <String, T>{{}};

  T? memo(String k) => _memo[k];

  void remember(String k, T v) {{
    _memo[k] = v;
  }}
}}

class MPinned{i} extends Store<int> with MStore{i}<int> {{
  int total() {{
    var t = 0;
    for (final v in _memo.values) {{
      t += v;
    }}
    return t;
  }}
}}

String use{i}() {{
  final s = MPinned{i}();
  s.remember('a', 3);
  return '${{s.total() + s.size}}';
}}
""",
    )


def _t_trace(seed: int, i: int) -> str:
    return dart_file(
        seed,
        f"""
class TB{i} {{
  int calc(int x) => x * 2;
}}

mixin TTrace{i} on TB{i} {{
  @override
  int calc(int x) => super.calc(x) + 1;
}}

class TRun{i} extends TB{i} with TTrace{i} {{}}

String use{i}() => '${{TRun{i}().calc(10)}}';
""",
    )


def _t_cross(seed: int, i: int, k: int) -> str:
    return dart_file(
        seed,
        f"""
import 'package:mix_00/mix/provider_{k}.dart';

class RX{i} extends MixBase{k} with Mix{k}, MixTrace{k} {{}}

String use{i}() => '${{RX{i}().baseOp{k}(3)}}';
""",
    )


_SELF_FN = {
    "simple": _t_simple,
    "chain2": _t_chain2,
    "generic": _t_generic,
    "iface": _t_iface,
    "pointext": _t_pointext,
    "store": _t_store,
    "trace": _t_trace,
}


def _provider(seed: int, k: int) -> str:
    return dart_file(
        seed,
        f"""
class MixBase{k} {{
  int baseOp{k}(int x) => x + {k};

  int get baseVal => {k};
}}

mixin Mix{k} on MixBase{k} {{
  int mixedOp{k}() => baseOp{k}({k});
}}

mixin MixTrace{k} on MixBase{k} {{
  @override
  int baseOp{k}(int x) => super.baseOp{k}(x) + 1;
}}
""",
    )


def _wire(seed: int, pkg_index: int) -> str:
    if pkg_index == 0:
        # Mix0 is a mixin: instantiate via a tiny application class.
        body = """
import 'mix/provider_0.dart';
import 'mix/s000.dart';

class _WireApp extends MixBase0 with Mix0, MixTrace0 {}

String wire() => '${_WireApp().mixedOp0()}|${use0()}';
"""
    else:
        body = """
import 'mix/s000.dart';
import 'mix/s001.dart';

String wire() => use0() + use1();
"""
    return dart_file(seed, body)


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg not in MIX_PKGS:
        return None
    pkg_index = MIX_PKGS.index(pkg)
    out: dict[str, str] = {}
    if pkg_index == 0:
        for k in range(PROVIDERS):
            out[f"packages/{pkg}/lib/mix/provider_{k}.dart"] = _provider(seed, k)
    for idx in range(FILES_PER_PKG):
        if pkg_index == 0:
            template = SELF_TEMPLATES[idx % len(SELF_TEMPLATES)]
            body = _SELF_FN[template](seed, idx)
        elif idx % 4 == 0:
            body = _t_cross(seed, idx, idx % PROVIDERS)
        else:
            template = SELF_TEMPLATES[idx % len(SELF_TEMPLATES)]
            body = _SELF_FN[template](seed, idx)
        out[f"packages/{pkg}/lib/mix/s{idx:03d}.dart"] = body
    out[f"packages/{pkg}/lib/wire.dart"] = _wire(seed, pkg_index)
    return out
