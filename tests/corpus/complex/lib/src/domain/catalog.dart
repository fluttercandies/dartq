/// Static catalog tables: const collections at depth (maps of lists of
/// const records-free plain values), plus the runtime lookup surface.
library;

import '../core/identifiers.dart';
import 'product.dart';

/// Default merchandising tags per category; a fully const nested map.
const Map<ProductCategory, List<String>> defaultTagsByCategory = {
  ProductCategory.beverage: ['chilled', 'returnable'],
  ProductCategory.snack: ['impulse', 'front-shelf'],
  ProductCategory.household: ['bulk-aisle'],
  ProductCategory.personalCare: ['locked-cabinet'],
};

/// Regions served, ordered by opening date; const set literals dedupe.
const Set<String> servedRegions = {'north', 'south', 'port', 'inland'};

/// Priority SKU lanes; the leading zeros are gone but the separators keep
/// the digit groups readable (digit separators, stable since 3.6).
const Map<String, int> replenishmentLaneBySkuPrefix = {
  'BLK': 1_000_000,
  'FRSH': 250_000,
  'STD': 60_000,
};

/// Runtime catalog: built once per process from the static tables above.
class Catalog {
  final Map<SkuId, Product> bySku;

  Catalog(this.bySku);

  const Catalog.empty() : bySku = const {};

  Iterable<Product> get products => bySku.values;

  List<Product> inCategory(ProductCategory category) => [
        for (final product in bySku.values)
          if (product.variant.category == category) product,
      ];

  /// Hazard-adjacent SKUs for the safety binder; uses spread-merge of two
  /// const sources with a collection filter.
  List<String> hazardShortlist() {
    final hazardous = [
      for (final product in bySku.values)
        if (product.needsHazardCertification) product.variant.sku.value,
    ];
    return [
      ...hazardous,
      ...?defaultTagsByCategory[ProductCategory.household],
    ];
  }

  /// The largest replenishment lane whose prefix matches [sku], or null.
  int? laneFor(SkuId sku) {
    int? best;
    for (final entry in replenishmentLaneBySkuPrefix.entries) {
      if (sku.value.startsWith(entry.key)) {
        final lane = entry.value;
        if (best == null || lane > best) {
          best = lane;
        }
      }
    }
    return best;
  }
}
