/// Supplier domain: function-typed members, generic negotiation methods and
/// a covariant override pair.
library;

import '../core/identifiers.dart';
import '../core/money.dart';

/// Availability probe supplied by the integration layer.
typedef AvailabilityCheck = bool Function(Quantity requested);

class Supplier {
  final PurchaseRef purchaseRef;
  final String legalName;
  final int leadTimeDays;
  final AvailabilityCheck? availabilityCheck;

  Supplier({
    required this.purchaseRef,
    required this.legalName,
    required this.leadTimeDays,
    this.availabilityCheck,
  });

  bool get isReliable => leadTimeDays <= 14;

  bool canFulfill(Quantity requested) =>
      availabilityCheck?.call(requested) ?? requested.units > 0;

  /// Maps the supplier onto any negotiation outcome without casting at the
  /// call site: fully generic both directions.
  R negotiateWith<R>(R Function(Supplier supplier) negotiation) =>
      negotiation(this);

  @override
  String toString() => 'Supplier($legalName, ${leadTimeDays}d)';
}

/// Base merge target for the covariant-override pair below.
abstract class MergeableCatalog {
  void mergeFrom(MergeableCatalog other);

  int get entryCount;
}

/// Supplier-specific catalog: narrows the parameter with `covariant`, the
/// language-sanctioned checked narrowing in override position.
class SupplierCatalog extends MergeableCatalog {
  final Map<SkuId, Money> priceList = {};

  @override
  int get entryCount => priceList.length;

  void upsert(SkuId sku, Money price) => priceList[sku] = price;

  Money? priceFor(SkuId sku) => priceList[sku];

  @override
  void mergeFrom(covariant SupplierCatalog other) {
    priceList.addAll(other.priceList);
  }
}

/// A read-only view: overriding again with the same narrowing keeps the
/// family consistent for clients that hold the base type.
class FrozenSupplierCatalog extends SupplierCatalog {
  @override
  void mergeFrom(covariant SupplierCatalog other) {
    throw UnsupportedError('frozen catalog cannot merge');
  }
}
