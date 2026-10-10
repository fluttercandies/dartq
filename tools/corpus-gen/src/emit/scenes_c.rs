//! Matrix scenes: null safety, labeled jumps, async shapes, function types,
//! cascades, string interpolation, and numeric literals.

use crate::rng::SplitMix64;
use std::fmt::Write as _;

pub(super) fn null_safety(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: null safety (?, !, late, ??).\n");
    dw!(
        w,
        "class Cache{id} {{\n  late final String _key;\n\n  String? _value;\n\n  Cache{id}(String key) {{\n    _key = key;\n  }}\n\n  String? get value => _value;\n\n  set value(String? next) {{\n    _value = next;\n  }}\n\n  String require() => _value!;\n\n  String resolve(String fallback) {{\n    final current = _value ?? fallback;\n    return current.isEmpty ? fallback : current;\n  }}\n\n  int measure(Map<String, String?> source) {{\n    return source[_key]?.length ?? 0;\n  }}\n}}\n"
    );
}

pub(super) fn labeled_break(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: labeled break and continue.\n");
    dw!(
        w,
        "int scan{id}(List<List<int>> grid) {{\n  var hits = 0;\n  outer:\n  for (final row in grid) {{\n    for (final cell in row) {{\n      if (cell < 0) {{\n        continue outer;\n      }}\n      if (cell == 0) {{\n        break outer;\n      }}\n      hits += cell;\n    }}\n  }}\n  return hits;\n}}\n"
    );
}

pub(super) fn async_future(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: Future and await.\n");
    dw!(
        w,
        "Future<int> fetch{id}(int seed) async {{\n  await Future<void>.delayed(Duration.zero);\n  return seed + 1;\n}}\n\n"
    );
    dw!(
        w,
        "Future<List<int>> orchestrate{id}() async {{\n  final first = await fetch{id}(1);\n  final results = await Future.wait<int>(<Future<int>>[\n    fetch{id}(first),\n    fetch{id}(2),\n  ]);\n  return results;\n}}\n"
    );
}

pub(super) fn async_stream(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: Stream and async*.\n");
    dw!(
        w,
        "Stream<int> ticks{id}(int count) async* {{\n  for (var i = 1; i <= count; i++) {{\n    yield i;\n  }}\n}}\n\n"
    );
    dw!(
        w,
        "Future<int> drain{id}() async {{\n  var total = 0;\n  await for (final tick in ticks{id}(3)) {{\n    total += tick;\n  }}\n  return total;\n}}\n\n"
    );
    dw!(
        w,
        "Stream<String> mapped{id}(Stream<int> source) => source.map((value) => 'tick $value');\n"
    );
}

pub(super) fn typedef_function_type(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: typedefs and generic function types.\n");
    dw!(w, "typedef Reducer{id} = int Function(int state, int action);\n\n");
    dw!(w, "typedef Predicate<T> = bool Function(T value);\n\n");
    dw!(w, "typedef Formatter<T> = String Function(T value, {{String prefix}});\n\n");
    dw!(
        w,
        "int foldOver{id}(List<int> items, Reducer{id} reducer, int seed) {{\n  var state = seed;\n  for (final item in items) {{\n    state = reducer(state, item);\n  }}\n  return state;\n}}\n\n"
    );
    dw!(
        w,
        "bool anyMatch<T>(List<T> items, Predicate<T> test) {{\n  for (final item in items) {{\n    if (test(item)) {{\n      return true;\n    }}\n  }}\n  return false;\n}}\n\n"
    );
    dw!(w, "String show{id}<T>(T value, Formatter<T> format) => format(value);\n");
}

pub(super) fn cascade(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: cascades (.. and ?..).\n");
    dw!(
        w,
        "class Builder{id} {{\n  final List<String> parts = <String>[];\n\n  bool verbose = false;\n\n  Builder{id} add(String part) {{\n    parts.add(part);\n    return this;\n  }}\n\n  Builder{id} setVerbose(bool value) {{\n    verbose = value;\n    return this;\n  }}\n\n  String build() => parts.join('-');\n}}\n\n"
    );
    dw!(
        w,
        "String compose{id}() {{\n  final builder = Builder{id}()\n    ..add('alpha')\n    ..add('beta')\n    ..setVerbose(true);\n  final Builder{id}? maybe = Builder{id}()?..add('gamma');\n  return '${{builder.build()}}/${{maybe?.build() ?? 'empty'}}';\n}}\n"
    );
}

pub(super) fn string_interpolation(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: string interpolation and adjacent strings.\n");
    dw!(
        w,
        "String banner{id}(String name, int version) {{\n  final single = 'app=$name v$version';\n  final adjacent = 'first part '\n      'second part ${{name.toUpperCase()}} '\n      \"third part $single\";\n  final raw = r'temp${{not interpolated}}';\n  return '$adjacent (${{single.length}}) $raw';\n}}\n"
    );
}

pub(super) fn numbers(w: &mut String, id: u32, rng: &mut SplitMix64) {
    let extra = rng.below(0xF0);
    dw!(w, "\n/// Cell: numeric literals (decimal, hex, exponent).\n");
    dw!(
        w,
        "class Metrics{id} {{\n  static const int mask = 0xFF00;\n\n  static const double epsilon = 1.5e-8;\n\n  static const double halfPi = 3.14159e-1;\n\n  final int width;\n\n  final double scale;\n\n  const Metrics{id}(this.width, this.scale);\n\n  int get masked => width & mask;\n\n  double get scaled => width * scale + 2.5e2 + 0x{extra:X};\n}}\n"
    );
}
