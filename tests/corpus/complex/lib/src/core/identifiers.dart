/// Typed identifier surface: a generic alias plus extension types wrapping
/// the primitive representation, so identifier kinds cannot be mixed up.
library;

/// Opaque identifier alias: the type argument only documents the entity kind,
/// the representation stays a plain string.
typedef EntityId<T extends Object> = String;

/// Stock-keeping-unit identifier.
extension type SkuId(String value) {
  bool get isBulk => value.startsWith('BLK-');

  String shortForm() => value.length <= 8 ? value : value.substring(0, 8);
}

/// Sales-order identifier.
extension type OrderId(String value) {
  static final RegExp pattern = RegExp(r'^SO-\d{6}$');

  bool get isWellFormed => pattern.hasMatch(value);
}

/// Customer identifier.
extension type CustomerRef(String value) {
  const CustomerRef.anonymous() : value = 'anon';
}

/// Purchase-order identifier (procurement side of the domain).
extension type PurchaseRef(String value) {
  PurchaseRef.forOrder(OrderId id) : value = 'PO-${id.value}';
}

/// Helpers shared by every identifier kind: any extension type whose
/// representation is String can be visited through this generic extension.
extension IdInspection<T extends Object> on Iterable<EntityId<T>> {
  List<String> toSortedValues() {
    final values = map((id) => id.toString()).toList()..sort();
    return values;
  }
}
