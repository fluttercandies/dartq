/// Tax rules: named extensions with complex on-clauses — plain iterables,
/// nested map values and a fully generic grouped-flatten surface.
library;

import '../core/money.dart';

/// Jurisdiction ladder applied in order; each level has a basis.
enum TaxJurisdiction {
  federal(0.02),
  state(0.045),
  municipal(0.0125);

  const TaxJurisdiction(this.rate);

  final double rate;
}

extension TaxMath on Iterable<Money> {
  Money get total => fold(Money.zero, (sum, item) => sum + item);

  Money get average =>
      isEmpty ? Money.zero : Money((total.cents / length).round());
}

/// On-clause over a nested map-of-lists: extensions participate in
/// resolution whenever the receiver type matches, member-first.
extension RateLookup on Map<String, List<Money>> {
  Money totalFor(String zone) {
    final bucket = this[zone];
    if (bucket == null) {
      return Money.zero;
    }
    return bucket.total;
  }

  Iterable<String> get nonEmptyZones =>
      keys.where((zone) => (this[zone] ?? const []).isNotEmpty);
}

/// Generic extension with two parameters and a bounded type argument in the
/// on-clause itself: applicability requires V to be a Money subtype.
extension GroupedTotals<K, V extends Money> on Map<K, Iterable<V>> {
  Map<K, Money> totalsPerKey() => {
        for (final entry in entries) entry.key: entry.value.total,
      };

  Money grandTotal() {
    var sum = Money.zero;
    for (final value in values) {
      sum += value.total;
    }
    return sum;
  }
}

/// Computes the compound tax for a base amount across the ladder.
Money compoundTax(Money base, List<TaxJurisdiction> ladder) {
  var running = base;
  var accumulated = Money.zero;
  for (final level in ladder) {
    final due = Money((running.cents * level.rate).round());
    accumulated += due;
    running = base + accumulated;
  }
  return accumulated;
}
