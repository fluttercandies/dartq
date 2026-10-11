/// Restock rule contracts. Kept deliberately small: the main corpus package
/// implements and consumes these across the package boundary.
library;

/// A rule that decides whether a SKU needs restocking for a given on-hand
/// quantity. Implementations must be pure: no I/O, no clock reads.
abstract interface class RestockRule {
  /// Human-readable rule name used in audit output.
  String get name;

  /// Returns the suggested order amount, or zero when no restock is needed.
  int evaluate(int onHand);
}

/// Reorders when on-hand quantity drops under a fixed floor.
final class FloorRule implements RestockRule {
  final int floor;
  final int batchSize;

  const FloorRule({required this.floor, this.batchSize = 25});

  @override
  String get name => 'floor<=[$floor]';

  @override
  int evaluate(int onHand) => onHand <= floor ? batchSize : 0;
}

/// Combines several rules and takes the largest suggestion.
final class CompositeRule implements RestockRule {
  final List<RestockRule> rules;

  const CompositeRule(this.rules);

  @override
  String get name => rules.map((r) => r.name).join(' & ');

  @override
  int evaluate(int onHand) {
    var best = 0;
    for (final rule in rules) {
      final suggestion = rule.evaluate(onHand);
      if (suggestion > best) {
        best = suggestion;
      }
    }
    return best;
  }
}
