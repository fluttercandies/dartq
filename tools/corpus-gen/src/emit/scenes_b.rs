//! Matrix scenes: records and object/list/map/relational/logical patterns,
//! collection literals with control flow, and cross-package core usage.

use crate::rng::SplitMix64;
use std::fmt::Write as _;

pub(super) fn pattern_object(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: object pattern in a switch expression.\n");
    dw!(
        w,
        "class Vec{id} {{\n  final int x, y;\n\n  const Vec{id}(this.x, this.y);\n}}\n\n"
    );
    dw!(
        w,
        "String render{id}(Object value) {{\n  return switch (value) {{\n    Vec{id}(x: 0, y: 0) => 'origin',\n    Vec{id}(x: var x, y: var y) when x == y => 'diagonal',\n    Vec{id}(:var x, :var y) => 'vec($x, $y)',\n    _ => 'other',\n  }};\n}}\n"
    );
}

pub(super) fn pattern_record(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: record pattern in if-case.\n");
    dw!(
        w,
        "String label{id}(Object value) {{\n  if (value case (int a, int b) when a < b) {{\n    return 'ascending';\n  }}\n  if (value case (x: final left, y: final right)) {{\n    return 'point($left, $right)';\n  }}\n  if (value case (var only,)) {{\n    return 'single $only';\n  }}\n  return 'other';\n}}\n"
    );
}

pub(super) fn pattern_list_map(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(
        w,
        "\n/// Cell: list and map patterns in a switch statement.\n"
    );
    dw!(
        w,
        "String classify{id}(Object value) {{\n  switch (value) {{\n    case []:\n      return 'empty';\n    case [int first, ...var rest] when rest.isEmpty:\n      return 'one:$first';\n    case <int>[_, _, _]:\n      return 'triple ints';\n    case {{'name': String name}}:\n      return 'named:$name';\n    case {{0: var head, 1: var tail}}:\n      return 'headtail:$head/$tail';\n    default:\n      return 'other';\n  }}\n}}\n"
    );
}

pub(super) fn pattern_relational(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: relational patterns.\n");
    dw!(
        w,
        "String rate{id}(int score) {{\n  return switch (score) {{\n    < 0 => 'invalid',\n    0 => 'zero',\n    > 90 => 'elite',\n    >= 70 && < 90 => 'strong',\n    _ => 'normal',\n  }};\n}}\n"
    );
}

pub(super) fn pattern_logical(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(w, "\n/// Cell: logical-or and logical-and patterns.\n");
    dw!(
        w,
        "String gate{id}(Object value) {{\n  return switch (value) {{\n    0 || -1 || -2 => 'boundary',\n    String s when s.isEmpty || s.length > 8 => 'string-edge',\n    int n when n >= 1 && n <= 9 => 'digit',\n    _ => 'other',\n  }};\n}}\n"
    );
}

pub(super) fn collection_control_flow(w: &mut String, id: u32, _rng: &mut SplitMix64) {
    dw!(
        w,
        "\n/// Cell: collection literals with control flow and spreads.\n"
    );
    dw!(
        w,
        "List<int> expand{id}(List<int> input, bool keepEven) {{\n  return [\n    for (final value in input)\n      if (keepEven ? value.isEven : value.isOdd) value * 2,\n    ...input.reversed,\n    ...?input.isEmpty ? const <int>[0] : null,\n  ];\n}}\n\n"
    );
    dw!(
        w,
        "Map<String, int> tally{id}(List<String> words) {{\n  return <String, int>{{\n    for (final word in words) word: word.length,\n    '__sentinel': 0,\n  }};\n}}\n\n"
    );
    dw!(
        w,
        "Set<String> initials{id}(List<String> names) {{\n  return <String>{{\n    for (final name in names)\n      if (name.isNotEmpty) name.substring(0, 1),\n  }};\n}}\n"
    );
}

pub(super) fn core_value_use(w: &mut String, id: u32, rng: &mut SplitMix64) {
    let k = rng.below(1_000);
    dw!(
        w,
        "\n/// Cell: cross-package usage of the shared core types.\n"
    );
    dw!(w, "final CoreValue sharedValue{id} = CoreValue({k});\n\n");
    dw!(
        w,
        "int consumeValues{id}(List<CoreValue> values) {{\n  var total = sharedValue{id}.id;\n  for (final value in values) {{\n    total += value.id;\n  }}\n  return total;\n}}\n"
    );
}
