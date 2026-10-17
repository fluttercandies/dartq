"""Class-modifier family: the cross-library modifier faces.

- mod_provider publishes the modifier universe: a sealed hierarchy, a base
  class, an interface class, an abstract interface, an abstract base, mixin
  classes, and a generic constraint over the base class.
- modc_00/modc_01 exercise every VALID cross-library face (extend base as
  base/final/sealed, implement interface, exhaustive switches over the
  provider's sealed hierarchy, base mixins, generic holders).
- errprobe is the clearly separated error subtree: one bounded, certain
  ERROR-probe per file (modifier violations outside the provider library,
  type errors, pattern errors, async errors). This subtree is what keeps the
  differential gate's oracle side non-vacuous.
"""

from __future__ import annotations

from torture_common import MODC_PKGS, PKG_CORE, PKG_ERRPROBE, PKG_MOD_PROVIDER, dart_file

MODC_FILES = 32


def _provider_files(seed: int) -> dict[str, str]:
    out: dict[str, str] = {}
    out[f"packages/{PKG_MOD_PROVIDER}/lib/mods/hierarchy.dart"] = dart_file(
        seed,
        """
sealed class Shape {
  const Shape();
}

final class Circle extends Shape {
  final double r;

  const Circle(this.r);
}

final class Square extends Shape {
  final double side;

  const Square(this.side);
}

final class Rect extends Shape {
  final double w;
  final double h;

  const Rect(this.w, this.h);
}
""",
    )
    out[f"packages/{PKG_MOD_PROVIDER}/lib/mods/widgets.dart"] = dart_file(
        seed,
        """
base class BaseWidget {
  const BaseWidget();

  int get weight => 1;
}

final class FrozenWidget {
  const FrozenWidget();
}

interface class IfaceWidget {
  const IfaceWidget();

  void render() {}
}

abstract base class SemiBase {
  const SemiBase();

  int step(int x);
}

mixin class MixinWidget {
  void ping() {}
}

final class Utility {
  const Utility();

  static int util(int x) => x * 2;
}
""",
    )
    out[f"packages/{PKG_MOD_PROVIDER}/lib/mods/contracts.dart"] = dart_file(
        seed,
        """
abstract interface class Contract {
  int measure(String key);

  String get unit;
}
""",
    )
    out[f"packages/{PKG_MOD_PROVIDER}/lib/mods/mixable.dart"] = dart_file(
        seed,
        """
mixin class MixinExtra {
  int bonus() => 7;
}
""",
    )
    out[f"packages/{PKG_MOD_PROVIDER}/lib/mods/constraints.dart"] = dart_file(
        seed,
        """
import 'widgets.dart';

class Holder<T extends BaseWidget> {
  final T item;

  const Holder(this.item);
}
""",
    )
    out[f"packages/{PKG_MOD_PROVIDER}/lib/mods/sealed_use.dart"] = dart_file(
        seed,
        """
import 'hierarchy.dart';

String localName(Shape s) => switch (s) {
      Circle() => 'circle',
      Square() => 'square',
      Rect() => 'rect',
    };

double localArea(Shape s) => switch (s) {
      Circle(:final r) => 3.14 * r * r,
      Square(:final side) => side * side,
      Rect(:final w, :final h) => w * h,
    };
""",
    )
    out[f"packages/{PKG_MOD_PROVIDER}/lib/mods/mods_index.dart"] = dart_file(
        seed,
        """
export 'contracts.dart';
export 'hierarchy.dart';
export 'widgets.dart';
""",
    )
    out[f"packages/{PKG_MOD_PROVIDER}/lib/wire.dart"] = dart_file(
        seed,
        """
import 'mods/hierarchy.dart';
import 'mods/sealed_use.dart';
import 'mods/widgets.dart';

String wire() {
  const c = Circle(2);
  const w = BaseWidget();
  return '${localName(c)}|${w.weight}|${Utility.util(3)}';
}
""",
    )
    return out


_MODC_FACES = 11


def _face(seed: int, i: int, face: int) -> str:
    if face == 0:  # extend base as base
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

base class Bordered{i} extends BaseWidget {{
  const Bordered{i}();

  @override
  int get weight => 2;
}}

String use{i}() => 'w=${{const Bordered{i}().weight}}';
""",
        )
    if face == 1:  # extend base as final
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

final class Pinned{i} extends BaseWidget {{
  const Pinned{i}();
}}

String use{i}() => 'p=${{const Pinned{i}() is BaseWidget}}';
""",
        )
    if face == 2:  # local sealed subclass of the provider base
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

sealed class Panel{i} extends BaseWidget {{
  const Panel{i}();
}}

final class PanelA{i} extends Panel{i} {{
  const PanelA{i}();
}}

final class PanelB{i} extends Panel{i} {{
  const PanelB{i}();
}}

String use{i}() {{
  final Panel{i} p = PanelA{i}();
  return switch (p) {{
    PanelA{i}() => 'a',
    PanelB{i}() => 'b',
  }};
}}
""",
        )
    if face == 3:  # implement interface class outside
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

class ViaIface{i} implements IfaceWidget {{
  const ViaIface{i}();

  @override
  void render() {{}}
}}

String use{i}() {{
  const ViaIface{i}().render();
  return 'rendered';
}}
""",
        )
    if face == 4:  # implement abstract interface outside
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/contracts.dart';

final class Audited{i} implements Contract {{
  const Audited{i}();

  @override
  int measure(String key) => key.length;

  @override
  String get unit => 'chars';
}}

String use{i}() =>
    '${{const Audited{i}().measure('abc')}}/${{const Audited{i}().unit}}';
""",
        )
    if face == 5:  # extend abstract base outside as base
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

base class SemiKid{i} extends SemiBase {{
  const SemiKid{i}();

  @override
  int step(int x) => x + 1;
}}

String use{i}() => '${{const SemiKid{i}().step(1)}}';
""",
        )
    if face == 6:  # use a final class from outside
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

String use{i}() {{
  const f = FrozenWidget();
  return 'frozen=${{f is FrozenWidget}}';
}}
""",
        )
    if face == 7:  # mix in mixin classes from outside
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/mixable.dart';
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

class Paged{i} with MixinWidget, MixinExtra {{}}

class Kid{i} extends MixinWidget {{}}

String use{i}() {{
  Paged{i}().ping();
  return '${{Paged{i}().bonus()}}/${{Kid{i}() is MixinWidget}}';
}}
""",
        )
    if face == 8:  # exhaustive switch over the provider's sealed hierarchy
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/hierarchy.dart';

String name{i}(Shape s) => switch (s) {{
      Circle() => 'circle',
      Square() => 'square',
      Rect() => 'rect',
    }};

String use{i}() => name{i}(const Circle(1));
""",
        )
    if face == 9:  # generic holder bound to the provider base
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/constraints.dart';
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

base class Boxed{i} extends BaseWidget {{
  const Boxed{i}();
}}

String use{i}() {{
  final Holder<Boxed{i}> h = Holder(Boxed{i}());
  final Holder<BaseWidget> w = Holder(const BaseWidget());
  return '${{h.item is Boxed{i}}}|${{w.item.weight}}';
}}
""",
        )
    if face == 10:  # local base mixin on the provider base
        return dart_file(
            seed,
            f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

base mixin Shaded{i} on BaseWidget {{
  int shade(int x) => x * 3;
}}

final class ShadedBox{i} extends BaseWidget with Shaded{i} {{
  const ShadedBox{i}();
}}

String use{i}() => '${{const ShadedBox{i}().shade(2)}}';
""",
        )
    raise AssertionError(face)


def _modc_wire(seed: int) -> str:
    return dart_file(
        seed,
        """
import 'consume/face_000.dart';
import 'consume/face_001.dart';

String wire() => use0() + use1();
""",
    )


_PROBES = [
    (
        "e00_final_extended",
        f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

class Oops00 extends FrozenWidget {{}}
""",
    ),
    (
        "e01_interface_extended",
        f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

class Oops01 extends IfaceWidget {{}}
""",
    ),
    (
        "e02_base_extended_plain",
        f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

class Oops02 extends BaseWidget {{}}
""",
    ),
    (
        "e03_base_implemented_plain",
        f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

class Oops03 implements BaseWidget {{}}
""",
    ),
    (
        "e04_sealed_extended",
        f"""
import 'package:{PKG_MOD_PROVIDER}/mods/hierarchy.dart';

class Oops04 extends Shape {{}}
""",
    ),
    (
        "e05_sealed_on_constraint",
        f"""
import 'package:{PKG_MOD_PROVIDER}/mods/hierarchy.dart';

mixin Oops05 on Shape {{}}
""",
    ),
    (
        "e06_base_mixed",
        f"""
import 'package:{PKG_MOD_PROVIDER}/mods/widgets.dart';

class Oops06 with BaseWidget {{}}
""",
    ),
    (
        "e07_type_args",
        """
final Map<String> bad7 = <String, int>{};

final List<int> bad7b = <String>['a'];
""",
    ),
    (
        "e08_undefined",
        """
final int missing8 = noSuchSymbolAnywhere;

void ghostCall8() {
  alsoMissing8();
}
""",
    ),
    (
        "e09_assign",
        """
int nine = 'not an int';

String nineB = 42;
""",
    ),
    (
        "e10_pattern_undefined_field",
        f"""
import 'package:{PKG_CORE}/geometry.dart';

String ten(Object o) => switch (o) {{
      Point(:var nope) => 'x',
      _ => 'y',
    }};
""",
    ),
    (
        "e11_map_pattern_nonconst_key",
        """
final Object k11 = Object();

String eleven(Object o) => switch (o) {
      {k11: var v} => 'matched',
      _ => 'no',
    };
""",
    ),
    (
        "e12_switch_nonexhaustive",
        f"""
import 'package:{PKG_MOD_PROVIDER}/mods/hierarchy.dart';

String twelve(Shape s) => switch (s) {{
      Circle() => 'c',
    }};
""",
    ),
    (
        "e13_await_in_sync",
        """
int thirteen() {
  await Future<int>.value(1);
  return 1;
}
""",
    ),
    (
        "e14_return_type",
        """
Future<String> fourteen() async {
  return 42;
}

String fourteenB() {
  return 42;
}
""",
    ),
    (
        "e15_const_nonconst",
        """
const String bad15 = DateTime.now().toString();
""",
    ),
    (
        "e16_record_pattern_arity",
        """
final (int a, int b) = (1, 2, 3);
""",
    ),
    (
        "e17_missing_ctor_arg",
        """
class NeedsInit17 {
  final int v;

  const NeedsInit17(this.v);
}

final NeedsInit17 bad17 = NeedsInit17();
""",
    ),
]


def emit(pkg: str, seed: int) -> dict[str, str] | None:
    if pkg == PKG_MOD_PROVIDER:
        return _provider_files(seed)
    if pkg in MODC_PKGS:
        out: dict[str, str] = {}
        for i in range(MODC_FILES):
            face = i % _MODC_FACES
            rel = f"packages/{pkg}/lib/consume/face_{i:03d}.dart"
            out[rel] = _face(seed, i, face)
        out[f"packages/{pkg}/lib/wire.dart"] = _modc_wire(seed)
        return out
    if pkg == PKG_ERRPROBE:
        out = {}
        for name, body in _PROBES:
            out[f"packages/{PKG_ERRPROBE}/lib/probe/{name}.dart"] = dart_file(seed, body)
        return out
    return None
