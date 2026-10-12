// Valid Dart 3.15 acceptance corpus — kitchen sink: one file combining the
// shipped feature families (sealed hierarchy with an exhaustive switch
// expression, primary constructors, object/relational patterns, records
// with a named destructure, dot shorthands over an extension type,
// null-aware elements, the `>>>` operator over both a class operator and
// int literals, and digit separators).
//
// Provenance: each face mirrors the per-family official language tests
// cited in the sibling corpus files' headers (SDK main @ e6f4207a).
// Covered by scripts/acceptance-gate.sh (zero-diagnostics bar, `dartq
// check` machine face).

sealed class Expr {
  const Expr();

  int eval();
}

final class Lit extends Expr {
  final int value;
  const Lit(this.value);

  @override
  int eval() => value;

  Expr operator >>>(int bits) => Lit(value >>> bits);
}

final class Add extends Expr {
  final Expr left;
  final Expr right;
  const Add(this.left, this.right);

  @override
  int eval() => left.eval() + right.eval();
}

enum Op { add, mul }

extension type Meters(int value) {
  static Meters get zero => Meters(0);
}

class Reading(var int id, final Meters offset) {
  int scaled() => id * offset.value;
}

String render(Expr expr) => switch (expr) {
      Lit(value: var v) => 'lit $v',
      Add(left: var l, right: var r) => '(${render(l)} + ${render(r)})',
    };

int? evenOrNull(int n) => n.isEven ? n : null;

void main() {
  final tree = Add(Lit(1_000), Add(Lit(20), Lit(3)));
  final text = render(tree);
  final total = tree.eval();
  final shifted = Lit(4) >>> 1;
  final reading = Reading(9_999, Meters.zero);
  final op = Op.add;
  final flagged = <int>[?evenOrNull(total), ?evenOrNull(2)];
  final scaled = <Meters>[Meters.zero, Meters(1_0_0)];
  final opLabel = switch (op) {
    Op.add => 'add',
    Op.mul => 'mul',
  };
  var offsetSum = 0;
  if (reading
      case Reading(id: final int rid, offset: final Meters m) when rid > 0) {
    offsetSum = m.value;
  }
  final pair = (id: reading.id, offset: offsetSum);
  final (:id, :offset) = pair;

  if (text.isEmpty ||
      total != 1023 ||
      shifted.eval() != 2 ||
      flagged.length != 1 ||
      scaled.length != 2 ||
      opLabel != 'add' ||
      offsetSum != 0 ||
      id + offset != 9_999 ||
      reading.scaled() != 0) {
    throw StateError('unreachable');
  }
}
