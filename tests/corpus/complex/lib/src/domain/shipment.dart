/// Shipping domain: the class-modifier matrix in one coherent model —
/// interface, base, final, sealed, mixin class and on-clause mixins.
library;

import '../core/identifiers.dart';
import '../core/money.dart';
import '../core/timestamps.dart';

/// Anything that can be shipped. Interface class: implementable from other
/// libraries, never extended there.
abstract interface class Shippable {
  Money get shippingCost;

  String manifestLine();
}

/// Physical tracking capability. Mixin class: both mixinable and
/// extensible, the 3.0 answer to "mixin that needs a constructor later".
mixin class Trackable {
  String? _trackingCode;

  String? get trackingCode => _trackingCode;

  void assignTracking(String code) {
    _trackingCode = code;
  }
}

/// Common shipment state. Base class: extendable within the library only;
/// other libraries may use it as a type but not subclass it.
abstract base class BaseShipment with Trackable implements Shippable {
  final SkuId sku;
  final Instant dispatchAt;

  BaseShipment({required this.sku, required this.dispatchAt});

  @override
  Money get shippingCost;

  @override
  String manifestLine() =>
      'SHIP ${sku.shortForm()} @ ${dispatchAt.secondsSinceEpoch}';
}

final class GroundShipment extends BaseShipment {
  final int groundSegments;

  GroundShipment({
    required super.sku,
    required super.dispatchAt,
    required this.groundSegments,
  });

  @override
  Money get shippingCost => Money(500 * groundSegments);
}

final class AirShipment extends BaseShipment {
  final bool refrigerated;

  AirShipment({
    required super.sku,
    required super.dispatchAt,
    required this.refrigerated,
  });

  @override
  Money get shippingCost => Money(refrigerated ? 12_000 : 7_500);
}

/// Refrigeration audit behavior; the on-clause keeps it applicable only to
/// base shipments, which keeps the manifest available. A base supertype
/// forces the `base` modifier on the mixin.
base mixin ColdChainAudit on BaseShipment {
  int get temperatureChecks;

  String auditTrail() =>
      'COLD n=$temperatureChecks ${manifestLine()}';
}

final class ReeferShipment extends BaseShipment with ColdChainAudit {
  final int temperatureChecks;

  ReeferShipment({
    required super.sku,
    required super.dispatchAt,
    required this.temperatureChecks,
  });

  @override
  Money get shippingCost => Money(9_900 + 250 * temperatureChecks);
}

/// Sealed scan lifecycle: same-library only, exhaustive below.
sealed class ShipmentScan {
  final Instant at;

  const ShipmentScan({required this.at});
}

final class PickedUpScan extends ShipmentScan {
  final String dock;

  const PickedUpScan({required super.at, required this.dock});
}

final class InTransitScan extends ShipmentScan {
  final String hub;

  const InTransitScan({required super.at, required this.hub});
}

final class DeliveredScan extends ShipmentScan {
  final String signedBy;

  const DeliveredScan({required super.at, required this.signedBy});
}

/// Human summary of a scan sequence; exhaustive switch expression with a
/// relational guard on the transit hop count. The subject list pattern
/// counts consecutive transit scans without a manual loop.
String describeScans(List<ShipmentScan> scans) {
  var transitHops = 0;
  for (final scan in scans) {
    if (scan case InTransitScan(:final hub)) {
      transitHops += hub.length > 0 ? 1 : 0;
    }
  }
  final last = scans.isEmpty ? null : scans.last;
  return switch (last) {
    null => 'no scans yet (hops $transitHops)',
    PickedUpScan(dock: final dock) => 'picked up at $dock',
    InTransitScan(hub: final hub) when transitHops > 2 =>
      'stuck near $hub after $transitHops hops',
    InTransitScan(hub: final hub) => 'in transit via $hub',
    DeliveredScan(signedBy: final signedBy) => 'delivered, signed $signedBy',
  };
}
