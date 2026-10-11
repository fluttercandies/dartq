/// Discount domain: a sealed rule hierarchy with pattern-driven application,
/// including guard clauses and nested object patterns.
library;

import '../core/money.dart';

/// Sealed discount hierarchy; every rule lives in this library so the
/// switches below are exhaustively checked without a default arm.
sealed class Discount {
  final String code;

  const Discount({required this.code});
}

final class PercentageOff extends Discount {
  final int percent;

  const PercentageOff({required this.percent}) : super(code: 'PCT-$percent');
}

final class AmountOff extends Discount {
  final Money amount;

  const AmountOff({required this.amount}) : super(code: 'AMT');
}

final class BulkThreshold extends Discount {
  final Quantity threshold;
  final PercentageOff reward;

  const BulkThreshold({
    required this.threshold,
    required this.reward,
  }) : super(code: 'BULK');
}

final class BundleDeal extends Discount {
  final List<String> bundledSkus;
  final int payForCount;

  const BundleDeal({
    required this.bundledSkus,
    required this.payForCount,
  }) : super(code: 'BUNDLE');
}

/// Applies a discount to a base amount. Exhaustive switch expression with a
/// guard clause; nested object pattern unwraps the bulk reward.
Money applyDiscount(Money base, Discount discount) => switch (discount) {
      PercentageOff(:final percent) =>
        base - Money(base.cents * percent ~/ 100),
      AmountOff(:final amount) => (base - amount).clampTo(Money.zero, base),
      BulkThreshold(threshold: final t, reward: PercentageOff(:final percent))
          when t.units >= 10 =>
        base - Money(base.cents * (percent + 5) ~/ 100),
      BulkThreshold(reward: final reward) => applyDiscount(base, reward),
      BundleDeal(bundledSkus: final skus) when skus.isEmpty => base,
      BundleDeal(:final payForCount) =>
        base - Money(base.cents * payForCount ~/ 100),
    };

/// Sorting-free priority: which discount a campaign should advertise first.
/// Logical-or pattern arms keep the ladder flat; the nested object pattern
/// keeps the relational on the integer field, not on Money itself.
String campaignHeadline(Discount discount) => switch (discount) {
      PercentageOff(percent: > 40) ||
      AmountOff(amount: Money(cents: > 5_000)) => 'Mega savings',
      PercentageOff() || AmountOff() => 'Save more today',
      BulkThreshold() => 'Buy more, save more',
      BundleDeal() => 'Bundle and save',
    };
