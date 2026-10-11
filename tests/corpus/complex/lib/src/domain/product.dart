/// Product domain: a sealed variant hierarchy with const constructors and a
/// composing aggregate. The exhaustive switch over the hierarchy lives in
/// the part file to exercise same-library exhaustiveness across parts.
library;

import '../core/identifiers.dart';
import '../core/money.dart';

part 'product_views.dart';

/// Merchandising category of a product line.
enum ProductCategory { beverage, snack, household, personalCare }

/// Sealed variant hierarchy: every variant lives in this library, so
/// consumer switches can be exhaustively checked.
sealed class ProductVariant {
  final SkuId sku;
  final ProductCategory category;
  final Money listPrice;

  const ProductVariant({
    required this.sku,
    required this.category,
    required this.listPrice,
  });
}

final class BeverageVariant extends ProductVariant {
  final int volumeMilliliters;
  final bool isCarbonated;

  const BeverageVariant({
    required super.sku,
    required super.listPrice,
    required this.volumeMilliliters,
    required this.isCarbonated,
  }) : super(category: ProductCategory.beverage);
}

final class SnackVariant extends ProductVariant {
  final int sugarGrams;

  const SnackVariant({
    required super.sku,
    required super.listPrice,
    required this.sugarGrams,
  }) : super(category: ProductCategory.snack);
}

final class HouseholdItemVariant extends ProductVariant {
  final bool isHazardous;

  const HouseholdItemVariant({
    required super.sku,
    required super.listPrice,
    required this.isHazardous,
  }) : super(category: ProductCategory.household);
}

/// The catalog aggregate: one purchasable product with its variant, stock
/// band and shelf placement.
class Product {
  final String title;
  final ProductVariant variant;
  final String shelfCode;

  const Product({
    required this.title,
    required this.variant,
    required this.shelfCode,
  });

  bool get needsHazardCertification => switch (variant) {
        HouseholdItemVariant(:final isHazardous) => isHazardous,
        _ => false,
      };

  @override
  String toString() => 'Product($title, ${variant.sku.value})';
}
