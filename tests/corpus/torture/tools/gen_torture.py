#!/usr/bin/env python3
"""gen_torture.py — deterministic generator for the torture corpus.

The torture corpus is the complexity-axis complement to tests/corpus/mega
(the size axis): one multi-package workspace exercising worst-case semantic
shapes (deep inheritance chains, mixin lattices, extension types,
cross-library class-modifier faces, every pattern form, F-bounded generics,
async-gap interleavings, cascades, tear-offs, typedefs, enhanced enums,
parts, conditional imports, nullability edges) with a clearly separated
error-probe package (errprobe) that keeps the differential gate's oracle
side non-vacuous.

Determinism contract: output is a pure function of (generator code, seed).
No RNG, no clock, no environment reads. Regenerating with the same seed into
a clean directory is byte-identical. The seed only feeds the file-header
comment and the manifest.

Usage:
  python3 tests/corpus/torture/tools/gen_torture.py [--out DIR] [--seed N]
          [--force]

  --out DIR   corpus root to write (default: tests/corpus/torture/corpus
              next to this tools/ directory). Refused when non-empty unless
              --force is given (a stale partial tree would break the
              byte-identical regeneration contract).
  --seed N    recorded in every generated header + the manifest (default 0).
  --force     wipe the output directory before generating.

Validation only (never run the corpus): `python3 -m py_compile` on the
tools/*.py files. Running analyzers over the generated tree is the gate's
serial-phase job (scripts/torture-gate.sh).
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import torture_async
import torture_chains
import torture_common as tc
import torture_core
import torture_exttypes
import torture_generics
import torture_misc
import torture_mixins
import torture_modifiers
import torture_patterns

FAMILY_OF: dict[str, str] = {}
for _pkg in tc.CHAIN_PKGS:
    FAMILY_OF[_pkg] = "chains"
for _pkg in tc.MIX_PKGS:
    FAMILY_OF[_pkg] = "mixins"
for _pkg in tc.MODC_PKGS:
    FAMILY_OF[_pkg] = "modifiers"
for _pkg in tc.EXTT_PKGS:
    FAMILY_OF[_pkg] = "exttypes"
for _pkg in tc.PAT_PKGS:
    FAMILY_OF[_pkg] = "patterns"
for _pkg in tc.GEN_PKGS:
    FAMILY_OF[_pkg] = "generics"
for _pkg in tc.ASYNC_PKGS:
    FAMILY_OF[_pkg] = "async"
for _pkg in tc.MISC_PKGS:
    FAMILY_OF[_pkg] = "misc"
FAMILY_OF[tc.PKG_CORE] = "core"
FAMILY_OF[tc.PKG_MOD_PROVIDER] = "modifiers"
FAMILY_OF[tc.PKG_ERRPROBE] = "modifiers"

FAMILY_MODULES = {
    "core": torture_core,
    "chains": torture_chains,
    "mixins": torture_mixins,
    "modifiers": torture_modifiers,
    "exttypes": torture_exttypes,
    "patterns": torture_patterns,
    "generics": torture_generics,
    "async": torture_async,
    "misc": torture_misc,
}


def package_deps() -> dict[str, list[str]]:
    """The deep dependency spine: every family head hangs off the previous
    family's tail, so the graph is one long chain with lateral edges into
    core_types. Cross-family references use package: imports; within a
    package, relative imports."""
    d: dict[str, list[str]] = {tc.PKG_CORE: []}
    d[tc.PKG_MOD_PROVIDER] = [tc.PKG_CORE]
    for i, pkg in enumerate(tc.CHAIN_PKGS):
        d[pkg] = [tc.PKG_CORE] + ([tc.CHAIN_PKGS[i - 1]] if i else [])
    for i, pkg in enumerate(tc.MIX_PKGS):
        d[pkg] = ([tc.MIX_PKGS[i - 1]] if i else [tc.CHAIN_PKGS[-1]]) + [tc.PKG_CORE]
    for i, pkg in enumerate(tc.MODC_PKGS):
        d[pkg] = [tc.PKG_MOD_PROVIDER, tc.PKG_CORE] + (
            [tc.MODC_PKGS[i - 1]] if i else []
        )
    for i, pkg in enumerate(tc.EXTT_PKGS):
        d[pkg] = ([tc.MIX_PKGS[-1]] if i == 0 else [tc.EXTT_PKGS[i - 1]]) + [
            tc.PKG_CORE
        ]
    for i, pkg in enumerate(tc.PAT_PKGS):
        d[pkg] = ([tc.EXTT_PKGS[-1]] if i == 0 else [tc.PAT_PKGS[i - 1]]) + [
            tc.PKG_CORE
        ]
    for i, pkg in enumerate(tc.GEN_PKGS):
        d[pkg] = ([tc.PAT_PKGS[-1]] if i == 0 else [tc.GEN_PKGS[i - 1]]) + [
            tc.PKG_CORE
        ]
    for i, pkg in enumerate(tc.ASYNC_PKGS):
        d[pkg] = ([tc.GEN_PKGS[-1]] if i == 0 else [tc.ASYNC_PKGS[i - 1]]) + [
            tc.PKG_CORE
        ]
    for i, pkg in enumerate(tc.MISC_PKGS):
        d[pkg] = ([tc.ASYNC_PKGS[-1]] if i == 0 else [tc.MISC_PKGS[i - 1]]) + [
            tc.PKG_CORE
        ]
    d[tc.PKG_ERRPROBE] = [tc.PKG_MOD_PROVIDER, tc.PKG_CORE]
    d[tc.PKG_APP] = [p for p in tc.ALL_PACKAGES if p != tc.PKG_ERRPROBE]
    return d


def _app_files(seed: int) -> dict[str, str]:
    """Root package: summaries that pull every family into one resolution
    unit plus a main() entry point."""
    out: dict[str, str] = {}

    def add(rel: str, body: str) -> None:
        out[f"lib/{rel}"] = tc.dart_file(seed, body)

    add(
        "summary/core.dart",
        f"""
import 'package:{tc.PKG_CORE}/colors.dart';
import 'package:{tc.PKG_CORE}/geometry.dart';
import 'package:{tc.PKG_CORE}/records_util.dart';

String coreSummary() =>
    '${{Point(1, 2).manhattan}}/${{Color.blue.label}}/${{describePair((3, 4))}}';
""",
    )
    add(
        "summary/chain.dart",
        f"""
import 'package:{tc.CHAIN_PKGS[-1]}/chain_a/level_255.dart';
import 'package:{tc.CHAIN_PKGS[-1]}/chain_b/level_255.dart';
import 'package:{tc.CHAIN_PKGS[-1]}/chain_c/level_255.dart';
import 'package:{tc.CHAIN_PKGS[-1]}/chain_d/level_255.dart';

String chainSummary() {{
  const b = ChainBL255(9);
  final d = ChainDL255<String>();
  return '${{ChainAL255().describe()}}|${{b.doubled}}|${{ChainCL255().label}}|${{d.payload('z')}}';
}}
""",
    )
    add(
        "summary/mixins.dart",
        f"""
import 'package:{tc.MIX_PKGS[-1]}/wire.dart';

String mixSummary() => wire();
""",
    )
    add(
        "summary/modifiers.dart",
        f"""
import 'package:{tc.PKG_MOD_PROVIDER}/mods/hierarchy.dart';
import 'package:{tc.PKG_MOD_PROVIDER}/wire.dart';

String modsSummary() {{
  const Shape s = Rect(2, 3);
  final name = switch (s) {{
    Circle() => 'c',
    Square() => 's',
    Rect() => 'r',
  }};
  return '${{wire()}}/$name';
}}
""",
    )
    add(
        "summary/modc.dart",
        f"""
import 'package:{tc.MODC_PKGS[-1]}/wire.dart';

String modcSummary() => wire();
""",
    )
    add(
        "summary/exttypes.dart",
        f"""
import 'package:{tc.EXTT_PKGS[-1]}/wire.dart';

String extSummary() => wire();
""",
    )
    add(
        "summary/patterns.dart",
        f"""
import 'package:{tc.PKG_CORE}/tokens.dart';

String patternsSummary() {{
  const Token t = NumT(6);
  return switch (t) {{
    WordT(:final text) => 'w$text',
    NumT(:final n) => 'n$n',
  }};
}}
""",
    )
    add(
        "summary/generics.dart",
        f"""
import 'package:{tc.GEN_PKGS[-1]}/wire.dart';

String genSummary() => wire();
""",
    )
    add(
        "summary/async.dart",
        f"""
import 'package:{tc.ASYNC_PKGS[-1]}/wire.dart';

String asyncSummary() => wire();

Future<String> asyncDetail() => wireAsync();
""",
    )
    add(
        "summary/misc.dart",
        f"""
import 'package:{tc.MISC_PKGS[-1]}/wire.dart';

String miscSummary() => wire();
""",
    )
    add(
        "main.dart",
        """
import 'summary/async.dart';
import 'summary/chain.dart';
import 'summary/core.dart';
import 'summary/exttypes.dart';
import 'summary/generics.dart';
import 'summary/misc.dart';
import 'summary/mixins.dart';
import 'summary/modc.dart';
import 'summary/modifiers.dart';
import 'summary/patterns.dart';

void main() {
  print(coreSummary());
  print(chainSummary());
  print(mixSummary());
  print(modsSummary());
  print(modcSummary());
  print(extSummary());
  print(patternsSummary());
  print(genSummary());
  print(asyncSummary());
  print(miscSummary());
}
""",
    )
    return out


def build_corpus(seed: int) -> dict[str, str]:
    deps = package_deps()
    files: dict[str, str] = {}
    for pkg in tc.ALL_PACKAGES:
        family = FAMILY_OF[pkg]
        emitted = FAMILY_MODULES[family].emit(pkg, seed)
        if emitted:
            files.update(emitted)
        desc = f"{pkg} package of the generated torture semantic stress corpus."
        files[f"packages/{pkg}/pubspec.yaml"] = tc.pubspec_text(pkg, desc, deps[pkg])
    app_desc = (
        "Root application package of the generated torture semantic stress corpus."
    )
    files["pubspec.yaml"] = tc.pubspec_text(tc.PKG_APP, app_desc, deps[tc.PKG_APP])
    files.update(_app_files(seed))
    files[".dart_tool/package_config.json"] = tc.package_config_text()
    files["MANIFEST.json"] = _manifest(files, seed)
    return files


def _manifest(files: dict[str, str], seed: int) -> str:
    per_pkg: dict[str, dict[str, int]] = {}
    total_loc = 0
    dart_files = 0
    for rel, text in files.items():
        parts = rel.split(os.sep)
        pkg = parts[1] if parts[0] == "packages" and len(parts) > 1 else tc.PKG_APP
        is_dart = rel.endswith(".dart")
        if is_dart:
            loc = text.count("\n")
            entry = per_pkg.setdefault(pkg, {"dart_files": 0, "loc": 0})
            entry["dart_files"] += 1
            entry["loc"] += loc
            total_loc += loc
            dart_files += 1
        else:
            per_pkg.setdefault(pkg, {"dart_files": 0, "loc": 0})
    doc = {
        "generator": tc.GENERATOR_REL,
        "seed": seed,
        "dart_files": dart_files,
        "total_files": len(files),
        "total_loc": total_loc,
        "packages": {k: per_pkg[k] for k in sorted(per_pkg)},
    }
    return json.dumps(doc, indent=2, sort_keys=True) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--out",
        default=None,
        help="corpus root to write (default: <tools>/../corpus)",
    )
    parser.add_argument("--seed", type=int, default=0, help="recorded seed (default 0)")
    parser.add_argument(
        "--force", action="store_true", help="wipe a pre-existing output directory"
    )
    args = parser.parse_args()

    tools_dir = os.path.dirname(os.path.abspath(__file__))
    out = args.out or os.path.normpath(os.path.join(tools_dir, os.pardir, "corpus"))
    if os.path.isdir(out) and os.listdir(out):
        if not args.force:
            print(
                f"gen_torture: {out} is not empty; regenerate with --force "
                "(wipe + regen keeps the byte-identical contract)",
                file=sys.stderr,
            )
            return 2
        shutil.rmtree(out)
    elif os.path.isfile(out):
        print(f"gen_torture: {out} is a file", file=sys.stderr)
        return 2

    files = build_corpus(args.seed)
    tc.write_corpus(out, files)

    dart_count = sum(1 for rel in files if rel.endswith(".dart"))
    print(f"gen_torture: wrote {len(files)} files ({dart_count} dart) to {out}")
    print(f"gen_torture: seed={args.seed} loc={sum(f.count(chr(10)) for f in files.values())}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
