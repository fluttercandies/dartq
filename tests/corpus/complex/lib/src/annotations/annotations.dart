/// Domain annotations: const-constructible metadata classes and their
/// application across declarations, including adjacent metadata on one
/// member and metadata on the library directive itself.
@Stable(since: '1.2.0')
library;

/// Marks a declaration as reviewed; [ticket] records the tracking entry.
class ReviewRequired {
  final String reason;
  final String ticket;

  const ReviewRequired(this.reason, {this.ticket = 'unfiled'});
}

/// Marks a declaration as API-stable for downstream packages.
class Stable {
  final String since;

  const Stable({this.since = '1.0.0'});
}

/// Declares a measured performance envelope.
class Measured {
  final int budgetNanos;

  const Measured(this.budgetNanos);
}

/// Contract annotations applied to a representative surface: two adjacent
/// annotations on one class, parameter-level metadata, field metadata and
/// metadata on a type parameter.
@Stable()
@ReviewRequired('hot path touched by every checkout', ticket: 'WH-402')
class PricingAnnotations {
  @Measured(1_500)
  int computeSurcharge(
    @ReviewRequired('fee schedule is regulatory') int feeSchedule,
    int baseCents,
  ) {
    return feeSchedule * baseCents;
  }

  /// Metadata on a type parameter keeps generic contracts self-describing.
  Map<String, int> surchargeTable<@Stable() T extends num>(List<T> tiers) => {
        for (var i = 0; i < tiers.length; i++) 'tier-$i': i * 100,
      };
}
