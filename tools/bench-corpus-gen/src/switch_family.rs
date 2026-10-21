//! Family A — switch/match-heavy libraries (the flow/pattern stress face).
//!
//! Each file declares several self-contained sealed hierarchies plus switch
//! expressions/statements over them: object, list, map, record, relational,
//! cast and logical-or patterns, if-case chains, guards, and exhaustive
//! switches over sealed types and enums. Arm sets are written so every
//! switch expression is exhaustive without redundant wildcard arms and every
//! switch statement leaves its `default` reachable — the two shapes that
//! would otherwise surface analyzer warnings in the oracle-clean contract.

use crate::rng::SplitMix64;
use crate::{File, sub};

/// Distinct section shapes rotated across sections and files.
pub const SHAPES: usize = 8;

const TAGS: [&str; 6] = ["hot", "cold", "core", "rim", "axial", "plain"];

fn hierarchy(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
sealed class Node::f_::s {
  const Node::f_::s();

  int baseValue() => 0;
}

final class Leaf::f_::s extends Node::f_::s {
  final int weight;
  final String tag;

  const Leaf::f_::s(this.weight, this.tag);

  @override
  int baseValue() => weight;
}

final class Pair::f_::s extends Node::f_::s {
  final Node::f_::s left;
  final Node::f_::s right;

  const Pair::f_::s(this.left, this.right);

  @override
  int baseValue() => left.baseValue() + right.baseValue();
}

final class Wrap::f_::s extends Node::f_::s {
  final List<Node::f_::s> items;
  final Map<String, int> scores;

  const Wrap::f_::s(this.items, this.scores);

  @override
  int baseValue() => items.length + scores.length;
}

",
        &binds,
    )
}

fn shape0(file: usize, section: usize, tag: &str) -> String {
    let binds = [
        ("::f", file.to_string()),
        ("::s", section.to_string()),
        ("::t", tag.to_string()),
    ];
    sub(
        "\
int weigh::f_::s(Node::f_::s node) {
  return switch (node) {
    Leaf::f_::s(:final weight) when weight > 10 => weight * 2,
    Leaf::f_::s(tag: '::t') => 3,
    Leaf::f_::s() => 1,
    Pair::f_::s(:final left, :final right) =>
      weigh::f_::s(left) + weigh::f_::s(right),
    Wrap::f_::s(items: [var first, ...var rest]) =>
      first.baseValue() + rest.length,
    Wrap::f_::s(:final scores) => scores['total'] ?? 0,
  };
}

",
        &binds,
    )
}

fn shape1(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
String classify::f_::s(Node::f_::s node, Mode::f mode) {
  var out = '';
  switch (node) {
    case Leaf::f_::s(:final weight) when weight > 100:
      out = 'heavy:$weight';
    case Leaf::f_::s(:final tag):
      out = 'light:$tag';
    case Pair::f_::s():
      out = 'pair:$mode';
    default:
      out = 'wrap:$mode';
  }
  return out;
}

",
        &binds,
    )
}

fn shape2(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
int stageLoad::f_::s(Stage::f stage, (int, String) pair) {
  var (count, label) = pair;
  return switch (stage) {
    Stage::f.cold => count,
    Stage::f.warm => count + label.length,
    Stage::f.hot => count * 2 + stage.burn,
  };
}

",
        &binds,
    )
}

fn shape3(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
String route::f_::s(Node::f_::s node) {
  if (node case Leaf::f_::s(:final tag) when tag.startsWith('a')) {
    return 'a:$tag';
  }
  if (node case Pair::f_::s(left: Leaf::f_::s(:final weight))) {
    return 'pl:$weight';
  }
  if (node case Wrap::f_::s(scores: {'run': var run})) {
    return 'run:$run';
  }
  return 'plain';
}

",
        &binds,
    )
}

fn shape4(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
int tally::f_::s(Wrap::f_::s wrap) {
  var total = 0;
  for (final item in wrap.items) {
    switch (item) {
      case Leaf::f_::s(:final weight):
        total += weight;
      case Pair::f_::s(:final left) when left.baseValue() > 0:
        total += 2;
      case Pair::f_::s():
        total += 1;
      case Wrap::f_::s():
        total += wrap.scores.length;
    }
  }
  if (wrap.scores case {'total': var t}) {
    total += t;
  }
  return total;
}

",
        &binds,
    )
}

fn shape5(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
String describe::f_::s(Node::f_::s node) {
  final text = switch (node) {
    Leaf::f_::s(:final weight, :final tag) => 'leaf($weight,$tag)',
    Pair::f_::s(:final left) => 'pair(${left.baseValue()})',
    Wrap::f_::s(:final items) when items.isEmpty => 'empty-wrap',
    Wrap::f_::s(items: List(isNotEmpty: true)) => 'full-wrap',
    Wrap::f_::s() => 'wrap',
  };
  return text;
}

",
        &binds,
    )
}

fn shape6(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
int bucket::f_::s(Object value) {
  return switch (value) {
    int n when n >= 1000 => 4,
    int n when n >= 100 => 3,
    int() => 2,
    String s when s.length > 8 => 1,
    String() => 0,
    List<int> xs => xs.length,
    Map<String, int> m => m.length,
    _ => -1,
  };
}

",
        &binds,
    )
}

fn shape7(file: usize, section: usize) -> String {
    let binds = [("::f", file.to_string()), ("::s", section.to_string())];
    sub(
        "\
String level::f_::s(int score, Node::f_::s node) {
  final grade = switch (score) {
    < 0 => 'neg',
    0 => 'zero',
    > 0 && < 10 => 'small',
    _ => 'big',
  };
  final kind = switch ((node, score)) {
    (Leaf::f_::s(:final tag), > 50) => 'hot:$tag',
    (Leaf::f_::s(), _) => 'leaf',
    (Pair::f_::s(), _) => 'pair',
    (Wrap::f_::s(), _) => 'wrap',
  };
  return '$grade/$kind';
}

",
        &binds,
    )
}

fn render_file(file: usize, target_lines: usize, rng: &mut SplitMix64) -> String {
    let binds = [("::f", file.to_string())];
    let mut body = sub(
        "\
// Switch-heavy synthetic library (family A, file ::f) from bench-corpus-gen.
// Sealed hierarchies with exhaustive switch expressions, guarded pattern
// switch statements, if-case chains and list/map/record/relational patterns.

enum Mode::f { alpha, beta, gamma, delta }

enum Stage::f {
  cold(1),
  warm(2),
  hot(3);

  const Stage::f(this.burn);

  final int burn;
}

",
        &binds,
    );
    let mut section = 0usize;
    while crate::count_lines(&body) < target_lines {
        let shape = (file * 3 + section) % SHAPES;
        let tag = TAGS[rng.below(TAGS.len() as u64) as usize];
        body.push_str(&hierarchy(file, section));
        let piece = match shape {
            0 => shape0(file, section, tag),
            1 => shape1(file, section),
            2 => shape2(file, section),
            3 => shape3(file, section),
            4 => shape4(file, section),
            5 => shape5(file, section),
            6 => shape6(file, section),
            _ => shape7(file, section),
        };
        body.push_str(&piece);
        section += 1;
    }
    body
}

/// Renders switch-heavy files until `budget_lines` is met. Returns the files
/// plus the emitted line count (>= budget, within one section overshoot).
pub fn render_switch_family(budget_lines: usize, rng: &mut SplitMix64) -> (Vec<File>, usize) {
    let mut files = Vec::new();
    let mut lines = 0usize;
    let mut index = 0usize;
    while lines < budget_lines {
        let regular = if index % 5 == 4 { 2000 } else { 200 };
        let target = regular.min(budget_lines - lines + 1);
        let body = render_file(index, target.max(40), rng);
        lines += crate::count_lines(&body);
        files.push(File {
            path: format!("lib/src/switch/switch_{index:04}.dart"),
            body,
        });
        index += 1;
    }
    (files, lines)
}
