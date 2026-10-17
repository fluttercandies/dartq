"""Inheritance chain family: four 256-level class hierarchies.

Level L of every chain lives in package chain_{L % 8}, so consecutive levels
are always in different packages: each of the 4 x 255 extends edges is a
cross-library subtype face. Chain shapes:

- a: plain override chain (describe/level + a per-level method)
- b: const constructor chain forwarding via super parameters
- c: implements an interface from core_types at every level
- d: generic <T> threaded through every level

These deliberately exceed the 20+ level bar by an order of magnitude to
stress transitive resolution.
"""

from __future__ import annotations

from torture_common import CHAIN_PKGS, PKG_CORE, dart_file

CHAIN_LETTERS = ("a", "b", "c", "d")
LEVELS = 256


def _level_a(seed: int, level: int) -> str:
    n = f"{level:03d}"
    if level == 0:
        body = f"""
class ChainAL{n} {{
  String describe() => 'chain a level {n}';
  int get level => 0;
  void step{n}() {{}}
}}
"""
    elif level % 16 == 0:
        prev = f"{level - 1:03d}"
        body = f"""
abstract class ChainAL{n} extends ChainAL{prev} {{
  @override
  int get level => {level};
}}
"""
    else:
        prev = f"{level - 1:03d}"
        body = f"""
class ChainAL{n} extends ChainAL{prev} {{
  @override
  String describe() => 'chain a level {n}';
  @override
  int get level => {level};
  void step{n}() {{}}
}}
"""
    if level == 0:
        return dart_file(seed, body)
    prev_pkg = CHAIN_PKGS[(level - 1) % len(CHAIN_PKGS)]
    imp = f"import 'package:{prev_pkg}/chain_a/level_{level - 1:03d}.dart';"
    return dart_file(seed, imp + "\n" + body)


def _level_b(seed: int, level: int) -> str:
    n = f"{level:03d}"
    if level == 0:
        body = f"""
class ChainBL{n} {{
  final int seed;

  const ChainBL{n}(this.seed);

  int get doubled => seed * 2;
}}
"""
    else:
        prev = f"{level - 1:03d}"
        body = f"""
class ChainBL{n} extends ChainBL{prev} {{
  const ChainBL{n}(super.seed);

  int get tag{n} => seed + {level};
}}
"""
    if level == 0:
        return dart_file(seed, body)
    prev_pkg = CHAIN_PKGS[(level - 1) % len(CHAIN_PKGS)]
    imp = f"import 'package:{prev_pkg}/chain_b/level_{level - 1:03d}.dart';"
    return dart_file(seed, imp + "\n" + body)


def _level_c(seed: int, level: int) -> str:
    n = f"{level:03d}"
    imp = f"import 'package:{PKG_CORE}/identified.dart';"
    if level == 0:
        body = f"""
{imp}

class ChainCL{n} implements Identified {{
  @override
  int get id => 0;

  @override
  String get label => 'chain-c-{n}';
}}
"""
        return dart_file(seed, body)
    prev = f"{level - 1:03d}"
    prev_pkg = CHAIN_PKGS[(level - 1) % len(CHAIN_PKGS)]
    imp += f"\nimport 'package:{prev_pkg}/chain_c/level_{prev}.dart';"
    body = f"""
{imp}

class ChainCL{n} extends ChainCL{prev} {{
  @override
  int get id => {level};

  @override
  String get label => 'chain-c-{n}';
}}
"""
    return dart_file(seed, body)


def _level_d(seed: int, level: int) -> str:
    n = f"{level:03d}"
    if level == 0:
        body = f"""
class ChainDL{n}<T> {{
  T payload(T x) => x;

  T? hold;

  void keep(T x) {{
    hold = x;
  }}
}}
"""
    else:
        prev = f"{level - 1:03d}"
        body = f"""
class ChainDL{n}<T> extends ChainDL{prev}<T> {{
  @override
  T payload(T x) => x;

  T pick{n}(T a, T b) => a;
}}
"""
    if level == 0:
        return dart_file(seed, body)
    prev_pkg = CHAIN_PKGS[(level - 1) % len(CHAIN_PKGS)]
    imp = f"import 'package:{prev_pkg}/chain_d/level_{level - 1:03d}.dart';"
    return dart_file(seed, imp + "\n" + body)


def _wire(seed: int, pkg_index: int) -> str:
    n = f"{pkg_index:03d}"
    body = f"""
import 'chain_a/level_{n}.dart';
import 'chain_b/level_{n}.dart';
import 'chain_c/level_{n}.dart';
import 'chain_d/level_{n}.dart';

final ChainAL{n} kAnchorA = ChainAL{n}();
final ChainBL{n} kAnchorB = const ChainBL{n}(7);
final ChainCL{n} kAnchorC = ChainCL{n}();
final ChainDL{n}<int> kAnchorD = ChainDL{n}<int>();

String wire() =>
    '${{kAnchorA.describe()}}|${{kAnchorB.doubled}}|${{kAnchorC.label}}|${{kAnchorD.payload(7)}}';
"""
    return dart_file(seed, body)


_LEVEL_EMITTERS = {"a": _level_a, "b": _level_b, "c": _level_c, "d": _level_d}


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg not in CHAIN_PKGS:
        return None
    pkg_index = CHAIN_PKGS.index(pkg)
    out: dict[str, str] = {}
    for letter in CHAIN_LETTERS:
        emitter = _LEVEL_EMITTERS[letter]
        for level in range(LEVELS):
            if level % len(CHAIN_PKGS) != pkg_index:
                continue
            rel = f"packages/{pkg}/lib/chain_{letter}/level_{level:03d}.dart"
            out[rel] = emitter(seed, level)
    out[f"packages/{pkg}/lib/wire.dart"] = _wire(seed, pkg_index)
    return out
