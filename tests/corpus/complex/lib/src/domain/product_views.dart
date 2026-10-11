/// Part of the product library: view helpers that rely on same-library
/// exhaustiveness over the sealed [ProductVariant] hierarchy.
part of 'product.dart';

/// One-line merchandising description per variant; exhaustive switch
/// expression over the sealed hierarchy (no default arm needed).
String describeVariant(ProductVariant variant) => switch (variant) {
      BeverageVariant(:final volumeMilliliters, isCarbonated: final fizzy) =>
        'beverage ${volumeMilliliters}ml${fizzy ? ' (fizzy)' : ''}',
      SnackVariant(:final sugarGrams) when sugarGrams > 20 =>
        'snack (high sugar, ${sugarGrams}g)',
      SnackVariant(:final sugarGrams) => 'snack (${sugarGrams}g)',
      HouseholdItemVariant(isHazardous: true) => 'household item (hazardous)',
      HouseholdItemVariant() => 'household item',
    };

/// Shelf label line rendered for price tags; uses an object pattern with a
/// nested relational pattern on a numeric member.
String shelfLabel(Product product) {
  final variant = product.variant;
  return switch (variant) {
    BeverageVariant(sku: final SkuId sku, volumeMilliliters: > 1_000) =>
      '${sku.shortForm()} LARGE ${product.title}',
    _ => '${variant.sku.shortForm()} ${product.title}',
  };
}
