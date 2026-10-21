//! Family C — general language-mix libraries (the broad semantic face).
//!
//! Sections rotate through six self-contained shapes: bounded generics,
//! mixins with `on` constraints and mixin applications, extensions on
//! generic types with record returns, typedef'd function types with
//! closures, const/factory/operator classes, and pattern destructuring in
//! declarations (list, map, object, record, indexed for-loops). Everything
//! declared is public or referenced, so no unused-element or
//! unused-local-variable diagnostics can surface.

use crate::rng::SplitMix64;
use crate::{File, sub};

/// Distinct section shapes rotated across sections and files.
pub const SHAPES: usize = 6;

const KEYS: [&str; 3] = ["k", "m", "z"];

fn shape0(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
class Shelf::f_::s<T extends num> {
  final Map<String, T> cells;

  const Shelf::f_::s(this.cells);

  T? cell(String key) => cells[key];

  double total() {
    var acc = 0.0;
    for (final v in cells.values) {
      acc += v.toDouble();
    }
    return acc;
  }

  List<T> descending() {
    final vs = cells.values.toList();
    vs.sort((a, b) => b.compareTo(a));
    return vs;
  }
}

int shelfProbe::f_::s(Shelf::f_::s<int> shelf) {
  final picked = shelf.cell('a') ?? 0;
  return picked + shelf.total().toInt() + shelf.descending().length;
}

",
        &binds,
    )
}

fn shape1(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
mixin Counted::f_::s {
  int get count => 4;
}

class Bin::f_::s<T> with Counted::f_::s {
  final List<T> held;

  Bin::f_::s(this.held);

  @override
  int get count => held.length;

  T first() => held.first;
}

mixin Traced::f_::s on Bin::f_::s<int> {
  String get trace => 'held:$count';
}

class Ledger::f_::s = Bin::f_::s<int> with Traced::f_::s;

int audit::f_::s(Ledger::f_::s ledger) => ledger.count + ledger.trace.length;

",
        &binds,
    )
}

fn shape2(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
extension Zipped::f_::s<T> on List<T> {
  List<(T, T)> zipped::f_::s() {
    final out = <(T, T)>[];
    var i = 0;
    while (i + 1 < length) {
      out.add((this[i], this[i + 1]));
      i += 2;
    }
    return out;
  }
}

class Zip::f_::s {
  List<(int, int)> pairsOf(List<int> xs) => xs.zipped::f_::s();
}

typedef Named::f_::s = ({int id, String label});

String show::f_::s(Named::f_::s row) {
  final (:id, :label) = row;
  return '$id:$label';
}

",
        &binds,
    )
}

fn shape3(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
typedef Picker::f_::s<T> = T Function(int index);

(int, int) pickPair::f_::s(Picker::f_::s<int> pick) {
  final a = pick(1);
  final b = pick(a + 2);
  return (a, b);
}

int applyTwice::f_::s(int seed, int Function(int) f) {
  int inner(int v) => f(f(v));
  return inner(seed);
}

int drive::f_::s() {
  final pair = pickPair::f_::s((i) => i * 3);
  return applyTwice::f_::s(pair.$1, (v) => v + pair.$2);
}

",
        &binds,
    )
}

fn shape4(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
class Pt::f_::s {
  final int x;
  final int y;

  const Pt::f_::s(this.x, this.y);

  const Pt::f_::s.origin() : x = 0, y = 0;

  factory Pt::f_::s.span(int n) => Pt::f_::s(n, n * 2);

  static const Pt::f_::s zero = Pt::f_::s.origin();

  Pt::f_::s operator +(Pt::f_::s other) => Pt::f_::s(x + other.x, y + other.y);

  bool get isZero => x == 0 && y == 0;
}

int offset::f_::s(int step) {
  const base = Pt::f_::s.origin();
  final moved = base + Pt::f_::s.span(step);
  return moved.x + moved.y + (Pt::f_::s.zero.isZero ? 0 : 1);
}

",
        &binds,
    )
}

fn shape5(file: usize, section: usize, key: &str, value: usize) -> String {
    let binds = [
        ("::f", file.to_string()),
        ("::s", section.to_string()),
        ("::k", key.to_string()),
        ("::v", value.to_string()),
    ];
    sub(
        "\
class Tick::f_::s {
  final int unit;
  final String mark;

  const Tick::f_::s(this.unit, this.mark);
}

int unpack::f_::s(List<int> xs) {
  var acc = 0;
  final head = xs.isEmpty ? null : xs.first;
  if (head != null) {
    acc += head * 2;
  }
  final [u, v, ...rest] = xs;
  acc += u + v + rest.length;
  final {'::k': w} = const <String, int>{'::k': ::v};
  acc += w;
  final Tick::f_::s(unit: tu, mark: tm) = Tick::f_::s(3, 'tick');
  acc += tu + tm.length;
  for (final (i, n) in xs.indexed) {
    acc += i + n;
  }
  return acc;
}

",
        &binds,
    )
}

fn render_file(file: usize, target_lines: usize, rng: &mut SplitMix64) -> String {
    let mut body = format!(
        "// General language-mix synthetic library (family C, file {file}) from\n// bench-corpus-gen: generics, mixins, extensions, closures, records,\n// const constructors and declaration patterns.\n\n"
    );
    let mut section = 0usize;
    while crate::count_lines(&body) < target_lines {
        let shape = (file * 5 + section) % SHAPES;
        let piece = match shape {
            0 => shape0(file, section),
            1 => shape1(file, section),
            2 => shape2(file, section),
            3 => shape3(file, section),
            4 => shape4(file, section),
            _ => {
                let key = KEYS[rng.below(KEYS.len() as u64) as usize];
                let value = 1 + rng.below(8) as usize;
                shape5(file, section, key, value)
            }
        };
        body.push_str(&piece);
        section += 1;
    }
    body
}

/// Renders general-mix files until `budget_lines` is met. Returns the files
/// plus the emitted line count (>= budget, within one section overshoot).
pub fn render_general_family(budget_lines: usize, rng: &mut SplitMix64) -> (Vec<File>, usize) {
    let mut files = Vec::new();
    let mut lines = 0usize;
    let mut index = 0usize;
    while lines < budget_lines {
        let regular = if index % 5 == 4 { 2000 } else { 200 };
        let target = regular.min(budget_lines - lines + 1);
        let body = render_file(index, target.max(40), rng);
        lines += crate::count_lines(&body);
        files.push(File {
            path: format!("lib/src/general/general_{index:04}.dart"),
            body,
        });
        index += 1;
    }
    (files, lines)
}
