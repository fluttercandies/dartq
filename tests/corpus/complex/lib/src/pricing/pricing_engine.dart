/// Pricing engine: F-bounded generic rule builder plus a calculator over a
/// generic line hierarchy, with depth-3 type arguments in its buckets.
library;

import '../core/money.dart';

/// Pricing rule base: the type the calculator stores. Kept non-generic so
/// heterogeneous rule collections need no bound-satisfying type argument.
abstract class PricingRule {
  int get priority;

  bool appliesTo(Money lineBase);

  Money apply(Money lineBase);
}

/// F-bounded refinement: each concrete rule retunes to its own type, so
/// `retuned` returns the concrete rule without casts at call sites. The
/// bound `Tunable<Self>` is what makes `Self` usable as a return type.
abstract class Tunable<Self extends Tunable<Self>> extends PricingRule {
  Self retuned(int newPriority);
}

final class FlatMarkupRule extends Tunable<FlatMarkupRule> {
  @override
  final int priority;

  final int markupCents;

  FlatMarkupRule({required this.priority, required this.markupCents});

  @override
  bool appliesTo(Money lineBase) => lineBase.cents > 0;

  @override
  FlatMarkupRule retuned(int newPriority) =>
      FlatMarkupRule(priority: newPriority, markupCents: markupCents);

  @override
  Money apply(Money lineBase) => lineBase + Money(markupCents);
}

final class TieredRateRule extends Tunable<TieredRateRule> {
  @override
  final int priority;

  final double rate;

  final Money freeAllowance;

  TieredRateRule({
    required this.priority,
    required this.rate,
    required this.freeAllowance,
  });

  @override
  bool appliesTo(Money lineBase) => lineBase > freeAllowance;

  @override
  TieredRateRule retuned(int newPriority) => TieredRateRule(
        priority: newPriority,
        rate: rate,
        freeAllowance: freeAllowance,
      );

  @override
  Money apply(Money lineBase) {
    final taxable = lineBase - freeAllowance;
    return lineBase + Money((taxable.cents * rate).round());
  }
}

/// A priced line in the calculator.
abstract base class PricedLine {
  final String label;

  PricedLine({required this.label});

  Money get basePrice;
}

final class SingleLine extends PricedLine {
  final Money price;

  SingleLine({required super.label, required this.price});

  @override
  Money get basePrice => price;
}

final class BundledLine extends PricedLine {
  final List<Money> parts;

  BundledLine({required super.label, required this.parts});

  @override
  Money get basePrice =>
      parts.fold(Money.zero, (sum, part) => sum + part);
}

/// The calculator: rules bucketed per currency-code and priority tier.
/// The bucket type nests three type arguments deep.
class PriceCalculator {
  final Map<String, Map<int, List<PricingRule>>> _buckets = {};

  void addRule(String currencyCode, int tier, PricingRule rule) {
    final tierBucket =
        _buckets.putIfAbsent(currencyCode, () => <int, List<PricingRule>>{});
    tierBucket.putIfAbsent(tier, () => []).add(rule);
  }

  List<PricingRule> rulesFor(String currencyCode) => [
        for (final tierRules in (_buckets[currencyCode] ?? const {}).values)
          ...tierRules,
      ];

  Money price(PricedLine line, {required String currencyCode}) {
    var total = line.basePrice;
    for (final rule in rulesFor(currencyCode)) {
      if (rule.appliesTo(total)) {
        total = rule.apply(total);
      }
    }
    return total;
  }

  /// Folds the rule chain over [seed] with [step]; the accumulator type is
  /// free and [step] sees every registered rule in tier order.
  R foldRules<R>(
    R seed,
    R Function(R acc, PricingRule rule) step, {
    required String currencyCode,
  }) {
    var acc = seed;
    for (final rule in rulesFor(currencyCode)) {
      acc = step(acc, rule);
    }
    return acc;
  }
}
